use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read as _,
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use regex::Regex;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse and compile a ledger source file into a binary `.dop` archive.
    ///
    /// The output is a Protocol Buffers snapshot of the elaborated journal,
    /// compressed with deflate by default. Loading a `.dop` file is much
    /// faster than re-parsing the source, making it suitable for large ledgers
    /// that are queried repeatedly.
    Compile {
        /// Path for the output `.dop` file.
        #[arg(short, long)]
        output: PathBuf,
        /// Path to the root `.ledger` source file (may use `include`).
        source: PathBuf,
        /// Write raw (uncompressed) protobuf instead of the default deflate.
        #[arg(long, default_value_t = false)]
        no_compression: bool,
    },

    /// Print the running balance for every account, optionally filtered by account name.
    ///
    /// Accepts either a raw `.ledger` source file or a pre-compiled `.dop`
    /// file. Output is formatted with the commodity and value right-aligned,
    /// followed by the account name.
    ///
    /// By default, output is rendered in tree form with indentation. Pass
    /// `--flat` to revert to the classic single-line-per-account format.
    /// `PATTERN` is a case-insensitive regular expression matched against the
    /// account name. Plain substrings are valid regex and match as literals.
    /// Omit it to show all accounts.
    Balance {
        source: PathBuf,
        /// Optional case-insensitive regex filter on account names.
        pattern: Option<String>,
        /// Include only transactions on or after this date (YYYY-MM-DD).
        #[arg(long)]
        begin: Option<String>,
        /// Include only transactions on or before this date (YYYY-MM-DD).
        #[arg(long)]
        end: Option<String>,
        /// Include only cleared transactions.
        #[arg(long)]
        cleared: bool,
        /// Include only transactions tagged with this tag.
        #[arg(long)]
        tag: Option<String>,
        /// Collapse accounts deeper than N colon-separated levels into their parent.
        #[arg(long)]
        depth: Option<usize>,
        /// Print flat output (full account names, no indentation) instead of the
        /// default tree view.
        #[arg(long, default_value_t = false)]
        flat: bool,
        /// Output format: text (default), json, or csv.
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// List individual postings, optionally filtered by account name.
    ///
    /// `PATTERN` is a case-insensitive regular expression matched against the
    /// account name. Plain substrings are valid regex and match as literals.
    /// Omit it to list all postings.
    Register {
        source: PathBuf,
        pattern: Option<String>,
        /// Only include transactions on or after this date (YYYY-MM-DD).
        #[arg(long)]
        begin: Option<String>,
        /// Only include transactions on or before this date (YYYY-MM-DD).
        #[arg(long)]
        end: Option<String>,
        /// Include only cleared transactions.
        #[arg(long, default_value_t = false)]
        cleared: bool,
        /// Include only transactions tagged with this tag.
        #[arg(long)]
        tag: Option<String>,
        /// Output format: text (default), json, or csv.
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Re-emit the journal as canonical Ledger source text.
    ///
    /// Parses and resolves the source file, then prints each transaction in
    /// canonical Ledger format. Only `.ledger` source files are accepted;
    /// pre-compiled `.dop` files do not preserve the original transaction
    /// structure needed for faithful re-emission.
    Print {
        /// Path to the root `.ledger` source file.
        source: PathBuf,
    },

    /// List all accounts that appear in the journal, one per line.
    ///
    /// Output is sorted alphabetically. Pass `PATTERN` to restrict the list
    /// to accounts whose name contains the pattern (case-insensitive).
    Accounts {
        source: PathBuf,
        /// Optional case-insensitive substring filter on account names.
        pattern: Option<String>,
    },

    /// List all commodity symbols used in the journal, one per line.
    ///
    /// Output is sorted and deduplicated.
    Commodities { source: PathBuf },

    /// Print a summary of the journal: transaction count, unique accounts,
    /// unique commodities, and the date range covered.
    Stats { source: PathBuf },
}

/// The set of supported output formats for `balance` and `register`.
enum OutputFormat {
    Text,
    Json,
    Csv,
}

impl OutputFormat {
    /// Parse the format string, returning an error with valid options listed.
    fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match s {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            "csv" => Ok(OutputFormat::Csv),
            other => Err(format!(
                "unknown format {:?}; valid options are: text, json, csv",
                other
            )
            .into()),
        }
    }
}

/// Load a [`doppio::elaboration::Journal`] from either a compiled `.dop` file or a
/// raw source file, without performing the `proto::Journal → elaboration::Journal`
/// reverse conversion.
///
/// The file type is detected by extension:
/// - `.dop` — validate header, decompress (if needed), and decode protobuf directly.
/// - anything else — select a [`doppio::Frontend`] via
///   [`doppio::frontend_for_extension`], parse and elaborate, then convert the
///   resulting [`doppio::Journal`] to [`doppio::elaboration::Journal`] (the cheaper
///   forward direction).
fn load_proto_journal(
    path: &PathBuf,
) -> Result<doppio::elaboration::Journal, Box<dyn std::error::Error>> {
    if let Some("dop") = path.extension().and_then(|e| e.to_str()) {
        let mut f = File::open(path)?;
        doppio::read_dop(&mut f, path)
    } else {
        let ext = path.extension().and_then(|e| e.to_str());
        let frontend = doppio::frontend_for_extension(ext);
        let base_path = path.parent().unwrap_or(std::path::Path::new(""));
        let mut file = String::new();
        File::open(path)?.read_to_string(&mut file)?;
        let ast_journal = frontend.parse(&file, base_path, &doppio::file_opener)?;
        let hir: doppio::resolution::HIR = ast_journal.try_into()?;
        Ok(doppio::elaborate(hir)?)
    }
}

/// Truncate an account name to at most `depth` colon-separated components.
///
/// Returns a subslice of `account` ending at the position of the `depth`-th
/// colon, or the full string if it has fewer than `depth` components.
///
/// # Examples
///
/// ```
/// assert_eq!(truncate_account("Expenses:Food:Restaurants", 2), "Expenses:Food");
/// assert_eq!(truncate_account("Assets:Checking", 1), "Assets");
/// assert_eq!(truncate_account("Assets", 1), "Assets");
/// ```
fn truncate_account(account: &str, depth: usize) -> &str {
    let mut colon_pos = None;
    let mut count = 0;
    for (i, c) in account.char_indices() {
        if c == ':' {
            count += 1;
            if count == depth {
                colon_pos = Some(i);
                break;
            }
        }
    }
    match colon_pos {
        Some(pos) => &account[..pos],
        None => account,
    }
}

/// Compile an optional account-filter pattern into a [`Regex`].
///
/// If `pattern` is `None`, returns a regex that matches everything (`.*`).
/// Otherwise wraps the pattern with `(?i)` for case-insensitive matching.
/// Returns an error with a clear message if the regex is syntactically invalid.
fn build_pattern_regex(pattern: Option<String>) -> Result<Regex, Box<dyn std::error::Error>> {
    let raw = match pattern {
        Some(p) => format!("(?i){}", p),
        None => ".*".to_string(),
    };
    Regex::new(&raw).map_err(|e| format!("invalid account pattern: {e}").into())
}

/// `TransactionState` integer values from the proto enum, mirrored here so we
/// can match without importing the whole proto namespace everywhere.
const STATE_CLEARED: i32 = doppio::elaboration::TransactionState::Cleared as i32;

struct JournalFilter {
    pattern: Regex,
    begin_date: Option<chrono::NaiveDate>,
    end_date: Option<chrono::NaiveDate>,
    cleared: bool,
    tag: Option<String>,
}

impl JournalFilter {
    fn new(
        pattern: Option<String>,
        begin: Option<&str>,
        end: Option<&str>,
        cleared: bool,
        tag: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let pattern = build_pattern_regex(pattern)?;

        let begin_date = begin
            .map(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                    format!("invalid --begin date '{}': expected format YYYY-MM-DD", s)
                })
            })
            .transpose()?;

        let end_date = end
            .map(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .map_err(|_| format!("invalid --end date '{}': expected format YYYY-MM-DD", s))
            })
            .transpose()?;

        Ok(JournalFilter {
            pattern,
            begin_date,
            end_date,
            cleared,
            tag,
        })
    }

    /// Returns `true` if `txn` passes all active filters.
    fn matches_transaction(&self, txn: &doppio::elaboration::Transaction) -> bool {
        if self.cleared && txn.state != STATE_CLEARED {
            return false;
        }

        if let Some(ref t) = self.tag
            && !txn.tags.iter().any(|tag| tag == t)
            && !txn
                .postings
                .iter()
                .any(|p| p.tags.iter().any(|tag| tag == t))
        {
            return false;
        }

        if self.begin_date.is_some() || self.end_date.is_some() {
            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            let txn_date = unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64));
            if let Some(txn_date) = txn_date {
                if let Some(begin) = self.begin_date
                    && txn_date < begin
                {
                    return false;
                }
                if let Some(end) = self.end_date
                    && txn_date > end
                {
                    return false;
                }
            }
        }

        true
    }

    fn matches_account(&self, account: &str) -> bool {
        self.pattern.is_match(account)
    }
}

/// Look up the format string for `commodity` in the proto commodities map.
///
/// Returns `None` if the commodity has no declared format.
fn commodity_format<'a>(
    commodity: &str,
    commodities: &'a std::collections::HashMap<String, doppio::elaboration::CommodityProperties>,
) -> Option<&'a str> {
    commodities.get(commodity).and_then(|p| p.format.as_deref())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile {
            output,
            source,
            no_compression,
        } => {
            let ext = source.extension().and_then(|e| e.to_str());
            let frontend = doppio::frontend_for_extension(ext);
            let base_path = source.parent().unwrap_or(std::path::Path::new(""));
            let mut file = String::new();
            File::open(&source)?.read_to_string(&mut file)?;
            let ast_journal = frontend.parse(&file, base_path, &doppio::file_opener)?;
            let hir: doppio::resolution::HIR = ast_journal.try_into()?;
            let journal: doppio::elaboration::Journal = doppio::elaborate(hir)?;
            let compression = if no_compression {
                doppio::Compression::None
            } else {
                doppio::Compression::Deflate
            };
            let mut out_file = File::create(output)?;
            doppio::write_dop(&journal, &mut out_file, compression)?;
        }
        Commands::Register {
            source,
            pattern,
            begin,
            end,
            cleared,
            tag,
            format,
        } => {
            let format = OutputFormat::parse(&format)?;
            let filter =
                JournalFilter::new(pattern, begin.as_deref(), end.as_deref(), cleared, tag)?;
            let journal = load_proto_journal(&source)?;

            // Per-commodity running total across all matching postings.
            let mut running: BTreeMap<String, rust_decimal::Decimal> = BTreeMap::new();

            // Build an iterator over transactions filtered by cleared, date range, and tag.
            let filtered_txns: Vec<_> = journal
                .transactions
                .iter()
                .filter(|txn| filter.matches_transaction(txn))
                .collect();

            match format {
                OutputFormat::Text => {
                    for txn in &filtered_txns {
                        // txn.date is Unix epoch days (1970-01-01 = 0); convert back to a
                        // human-readable date string for display.
                        let date = epoch_days_to_string(txn.date);

                        for posting in txn.postings.iter() {
                            if !filter.matches_account(&posting.account) {
                                continue;
                            }

                            // Sort commodity keys for deterministic output (proto uses HashMap),
                            // then accumulate the running total before printing.
                            let mut sorted_commodities: Vec<_> = posting
                                .amount
                                .as_ref()
                                .map(|a| a.by_commodity.iter().collect::<Vec<_>>())
                                .unwrap_or_default();
                            sorted_commodities.sort_by_key(|(k, _)| k.as_str());

                            // Accumulate every commodity into the running total.
                            for (commodity, proto_amount) in &sorted_commodities {
                                let amount = doppio::decimal_from_proto(proto_amount);
                                *running.entry(commodity.to_string()).or_default() += amount;
                            }

                            let mut commodities_iter = sorted_commodities.into_iter();
                            if let Some((commodity, proto_amount)) = commodities_iter.next() {
                                let amount = doppio::decimal_from_proto(proto_amount);
                                let amount_str = display_amount(
                                    commodity,
                                    amount,
                                    commodity_format(commodity, &journal.commodities),
                                );
                                let running_str = display_amount(
                                    commodity,
                                    running.get(commodity.as_str()).copied().unwrap_or_default(),
                                    commodity_format(commodity, &journal.commodities),
                                );
                                println!(
                                    "{:<10}  {:<20}  {:<30}  {:>15}  {:>15}",
                                    date,
                                    txn.description.chars().take(20).collect::<String>(),
                                    posting.account,
                                    amount_str,
                                    running_str,
                                );
                            }
                            for (commodity, proto_amount) in commodities_iter {
                                let amount = doppio::decimal_from_proto(proto_amount);
                                let amount_str = display_amount(
                                    commodity,
                                    amount,
                                    commodity_format(commodity, &journal.commodities),
                                );
                                let running_str = display_amount(
                                    commodity,
                                    running.get(commodity.as_str()).copied().unwrap_or_default(),
                                    commodity_format(commodity, &journal.commodities),
                                );
                                println!(
                                    "{:<10}  {:<20}  {:<30}  {:>15}  {:>15}",
                                    "", "", "", amount_str, running_str,
                                );
                            }
                        }
                    }
                }
                OutputFormat::Json => {
                    let mut rows: Vec<serde_json::Value> = Vec::new();
                    for txn in &filtered_txns {
                        let date = epoch_days_to_string(txn.date);
                        for posting in txn.postings.iter() {
                            if !filter.matches_account(&posting.account) {
                                continue;
                            }
                            // Sort for deterministic JSON output.
                            let mut sorted_commodities: Vec<_> = posting
                                .amount
                                .as_ref()
                                .map(|a| a.by_commodity.iter().collect::<Vec<_>>())
                                .unwrap_or_default();
                            sorted_commodities.sort_by_key(|(k, _)| k.as_str());
                            for (commodity, proto_amount) in sorted_commodities {
                                let amount = doppio::decimal_from_proto(proto_amount);
                                *running.entry(commodity.clone()).or_default() += amount;
                                let running_total =
                                    running.get(commodity.as_str()).copied().unwrap_or_default();
                                rows.push(serde_json::json!({
                                    "date": date,
                                    "description": txn.description,
                                    "account": posting.account,
                                    "commodity": commodity,
                                    "amount": amount.to_string(),
                                    "running_total": running_total.to_string(),
                                }));
                            }
                        }
                    }
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                }
                OutputFormat::Csv => {
                    println!("date,description,account,commodity,amount,running_total");
                    for txn in &filtered_txns {
                        let date = epoch_days_to_string(txn.date);
                        for posting in txn.postings.iter() {
                            if !filter.matches_account(&posting.account) {
                                continue;
                            }
                            // Sort for deterministic CSV output.
                            let mut sorted_commodities: Vec<_> = posting
                                .amount
                                .as_ref()
                                .map(|a| a.by_commodity.iter().collect::<Vec<_>>())
                                .unwrap_or_default();
                            sorted_commodities.sort_by_key(|(k, _)| k.as_str());
                            for (commodity, proto_amount) in sorted_commodities {
                                let amount = doppio::decimal_from_proto(proto_amount);
                                *running.entry(commodity.clone()).or_default() += amount;
                                let running_total =
                                    running.get(commodity.as_str()).copied().unwrap_or_default();
                                println!(
                                    "{},{},{},{},{},{}",
                                    csv_field(&date),
                                    csv_field(&txn.description),
                                    csv_field(&posting.account),
                                    csv_field(commodity),
                                    amount,
                                    running_total,
                                );
                            }
                        }
                    }
                }
            }
        }
        Commands::Print { source } => {
            if let Some("dop") = source.extension().and_then(|e| e.to_str()) {
                return Err("print only works with source files; \
                     .dop binary archives do not preserve the original transaction structure"
                    .into());
            }
            let ext = source.extension().and_then(|e| e.to_str());
            let frontend = doppio::frontend_for_extension(ext);
            let base_path = source.parent().unwrap_or(std::path::Path::new(""));
            let mut file = String::new();
            File::open(&source)?.read_to_string(&mut file)?;
            let ast_journal: doppio::ast::Journal =
                frontend.parse(&file, base_path, &doppio::file_opener)?;
            let hir: doppio::resolution::HIR = ast_journal.try_into()?;
            doppio::write_ledger(hir.transactions(), &mut std::io::stdout())?;
        }
        Commands::Accounts { source, pattern } => {
            let journal = load_proto_journal(&source)?;
            let pattern = pattern.map(|p| p.to_lowercase()).unwrap_or_default();
            // Sort account names — proto uses HashMap, so we must sort explicitly.
            let mut accounts: Vec<&String> = journal.accounts.keys().collect();
            accounts.sort();
            for account in accounts {
                if account.to_lowercase().contains(&pattern) {
                    println!("{}", account);
                }
            }
        }
        Commands::Commodities { source } => {
            let journal = load_proto_journal(&source)?;
            let commodities: BTreeSet<&str> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| {
                    posting
                        .amount
                        .as_ref()
                        .map(|a| {
                            a.by_commodity
                                .keys()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();
            for commodity in commodities {
                println!("{}", commodity);
            }
        }
        Commands::Stats { source } => {
            let journal = load_proto_journal(&source)?;

            let commodities: BTreeSet<&str> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| {
                    posting
                        .amount
                        .as_ref()
                        .map(|a| {
                            a.by_commodity
                                .keys()
                                .map(String::as_str)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();

            // txn.date is Unix epoch days (1970-01-01 = 0).
            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            let first_date = journal.transactions.first().and_then(|txn| {
                unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64))
            });
            let last_date = journal.transactions.last().and_then(|txn| {
                unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64))
            });

            println!("Transactions: {}", journal.transactions.len());
            println!("Accounts:     {}", journal.accounts.len());
            println!("Commodities:  {}", commodities.len());
            match (first_date, last_date) {
                (Some(first), Some(last)) => {
                    println!("First date:   {}", first);
                    println!("Last date:    {}", last);
                }
                _ => {
                    println!("First date:   N/A");
                    println!("Last date:    N/A");
                }
            }
        }
        Commands::Balance {
            source,
            pattern,
            begin,
            end,
            cleared,
            tag,
            depth,
            flat,
            format,
        } => {
            let format = OutputFormat::parse(&format)?;
            let filter =
                JournalFilter::new(pattern, begin.as_deref(), end.as_deref(), cleared, tag)?;
            let journal = load_proto_journal(&source)?;

            // Balances keyed by owned account name so depth-truncation can
            // produce new strings that aren't borrowed from the journal.
            let mut balances: BTreeMap<String, BTreeMap<String, rust_decimal::Decimal>> =
                BTreeMap::new();

            for txn in journal.transactions.iter() {
                if !filter.matches_transaction(txn) {
                    continue;
                }

                for posting in txn.postings.iter() {
                    if !filter.matches_account(&posting.account) {
                        continue;
                    }
                    let account = match depth {
                        Some(d) => truncate_account(&posting.account, d).to_owned(),
                        None => posting.account.clone(),
                    };
                    if let Some(amount) = &posting.amount {
                        for (commodity, proto_amount) in &amount.by_commodity {
                            *(balances
                                .entry(account.clone())
                                .or_default()
                                .entry(commodity.clone())
                                .or_default()) += doppio::decimal_from_proto(proto_amount);
                        }
                    }
                }
            }

            match format {
                OutputFormat::Text => {
                    for (account, commodities) in balances.iter() {
                        let indent_depth = account.chars().filter(|&c| c == ':').count();
                        let label: &str = if flat || indent_depth == 0 {
                            account.as_str()
                        } else {
                            // Show only the last component in tree mode.
                            account
                                .rsplit_once(':')
                                .map(|(_, last)| last)
                                .unwrap_or(account.as_str())
                        };
                        let indent = if flat { 0 } else { indent_depth * 2 };
                        let prefix = " ".repeat(indent);

                        let mut commodities_iter = commodities.iter();
                        if let Some((commodity, value)) = commodities_iter.next() {
                            let balance = display_amount(
                                commodity,
                                *value,
                                commodity_format(commodity, &journal.commodities),
                            );
                            println!("{balance:>20}  {prefix}{label}");
                        }
                        for (commodity, value) in commodities_iter {
                            let balance = display_amount(
                                commodity,
                                *value,
                                commodity_format(commodity, &journal.commodities),
                            );
                            println!("{balance:>20}");
                        }
                    }
                }
                OutputFormat::Json => {
                    let rows: Vec<serde_json::Value> = balances
                        .iter()
                        .map(|(account, acct_balances)| {
                            let commodity_amounts: Vec<serde_json::Value> = acct_balances
                                .iter()
                                .map(|(commodity, amount)| {
                                    serde_json::json!({
                                        "commodity": commodity,
                                        "amount": amount.to_string(),
                                    })
                                })
                                .collect();
                            serde_json::json!({
                                "account": account,
                                "balances": commodity_amounts,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                }
                OutputFormat::Csv => {
                    println!("account,commodity,amount");
                    for (account, acct_balances) in balances.iter() {
                        for (commodity, amount) in acct_balances.iter() {
                            println!("{},{},{}", csv_field(account), csv_field(commodity), amount,);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Format an amount according to a commodity's declared format string.
///
/// The format encodes prefix/suffix position, thousand separator, decimal
/// separator, and decimal places. Falls back to `"COMMODITY VALUE"` if the
/// format string cannot be parsed.
///
/// Examples:
/// - `"$1,000.00"` → prefix `$`, thousands `,`, decimal `.`, 2 places
/// - `"1.000,00 EUR"` → suffix ` EUR`, thousands `.`, decimal `,`, 2 places
/// - `"100 USD"` → suffix ` USD`, no thousands, no decimal
fn format_amount(commodity: &str, value: rust_decimal::Decimal, format: &str) -> String {
    // Determine prefix vs suffix by scanning for digit/sign characters.
    // Everything before the first digit/sign is the prefix; after the last
    // digit is the suffix (including any space).
    let first_digit = format
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() || *c == '-')
        .map(|(i, _)| i);
    let last_digit = format
        .char_indices()
        .rfind(|(_, c)| c.is_ascii_digit())
        .map(|(i, _)| i);

    let (prefix, number_part, suffix) = match (first_digit, last_digit) {
        (Some(s), Some(e)) => (&format[..s], &format[s..=e], &format[e + 1..]),
        _ => return format!("{commodity} {value}"),
    };

    // Detect the decimal separator: the last `.` or `,` in the number portion,
    // if it is followed by exactly N non-separator digits.
    let (decimal_sep, thousand_sep, decimal_places) = detect_separators(number_part);

    apply_format(
        commodity,
        value,
        prefix,
        suffix,
        decimal_sep,
        thousand_sep,
        decimal_places,
    )
}

/// Returns `(decimal_sep, thousand_sep, decimal_places)` by inspecting the
/// example number in the format string.
fn detect_separators(number: &str) -> (Option<char>, Option<char>, usize) {
    // Find the last occurrence of '.' or ',' — that's the decimal separator.
    let last_dot = number.rfind('.');
    let last_comma = number.rfind(',');

    let (decimal_sep, decimal_places) = match (last_dot, last_comma) {
        (Some(di), Some(ci)) if di > ci => {
            // dot comes last → decimal separator is '.', thousands is ','
            let places = number.len() - di - 1;
            (Some('.'), places)
        }
        (Some(di), Some(ci)) if ci > di => {
            // comma comes last → decimal separator is ',', thousands is '.'
            let places = number.len() - ci - 1;
            (Some(','), places)
        }
        (Some(di), None) => {
            // Single '.' with nothing else. Per ledger convention, a lone
            // separator followed by exactly 3 digits (e.g. `1.000`) is a
            // thousands separator, not a decimal point.
            let trailing = number.len() - di - 1;
            if trailing == 3 {
                (None, 0) // treat as thousands sep; decimal_sep stays None
            } else {
                (Some('.'), trailing)
            }
        }
        (None, Some(ci)) => {
            // Same logic for a lone ',': `1,000` → thousands sep.
            let trailing = number.len() - ci - 1;
            if trailing == 3 {
                (None, 0)
            } else {
                (Some(','), trailing)
            }
        }
        _ => (None, 0),
    };

    let thousand_sep = match decimal_sep {
        Some('.') if number.contains(',') => Some(','),
        Some(',') if number.contains('.') => Some('.'),
        None if number.contains(',') => Some(','),
        None if number.contains('.') => Some('.'),
        _ => None,
    };

    (decimal_sep, thousand_sep, decimal_places)
}

/// Render `value` using the parsed format components.
fn apply_format(
    commodity: &str,
    value: rust_decimal::Decimal,
    prefix: &str,
    suffix: &str,
    decimal_sep: Option<char>,
    thousand_sep: Option<char>,
    decimal_places: usize,
) -> String {
    use rust_decimal::prelude::ToPrimitive as _;

    // Re-scale the decimal to the correct number of places.
    let scaled = value.round_dp(decimal_places as u32);
    let is_neg = scaled.is_sign_negative();
    let abs = scaled.abs();

    // Split into integer and fractional parts.
    let integer_part = abs.trunc().to_u64().unwrap_or(0);
    let frac_str = if decimal_places > 0 {
        // Produce the fractional digits by taking the remainder and padding.
        let frac = abs.fract();
        let multiplier = rust_decimal::Decimal::from(10u64.pow(decimal_places as u32));
        let frac_digits = (frac * multiplier).to_u64().unwrap_or(0);
        format!("{frac_digits:0>width$}", width = decimal_places)
    } else {
        String::new()
    };

    // Format integer part with optional thousand separator.
    let int_str = if let Some(sep) = thousand_sep {
        let s = integer_part.to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                out.push(sep);
            }
            out.push(ch);
        }
        out.chars().rev().collect::<String>()
    } else {
        integer_part.to_string()
    };

    // Assemble number string.
    let number = if decimal_places > 0 {
        format!("{int_str}{}{frac_str}", decimal_sep.unwrap_or('.'))
    } else {
        int_str
    };

    let sign = if is_neg { "-" } else { "" };

    // The prefix/suffix may already contain the commodity symbol. Use the
    // format's prefix/suffix as-is if non-empty, otherwise fall back.
    //
    // Sign placement: for prefix formats (e.g. `$`) the sign goes before the
    // prefix so the result is `-$100`, not `$-100`.
    if !prefix.is_empty() || !suffix.is_empty() {
        format!("{sign}{prefix}{number}{suffix}")
    } else {
        // No prefix/suffix in format (shouldn't happen, but safe fallback).
        format!("{sign}{number} {commodity}")
    }
}

/// Format an amount using the commodity's declared format if available,
/// otherwise fall back to `"COMMODITY VALUE"`.
fn display_amount(commodity: &str, value: rust_decimal::Decimal, fmt: Option<&str>) -> String {
    if let Some(fmt) = fmt {
        format_amount(commodity, value, fmt)
    } else {
        format!("{commodity} {value}")
    }
}

/// Convert Unix epoch days to a `YYYY-MM-DD` string.
fn epoch_days_to_string(days: i32) -> String {
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|epoch| epoch.checked_add_signed(chrono::Duration::days(days as i64)))
        .map(|d| d.to_string())
        .unwrap_or_else(|| "????-??-??".to_string())
}

/// Escape a field for CSV output.
///
/// If the value contains a comma, double-quote, or newline it is wrapped in
/// double-quotes with internal double-quotes doubled per RFC 4180.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
