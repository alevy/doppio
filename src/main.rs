use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read as _, Write as _},
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
    /// The output is a postcard-serialised, XZ-compressed snapshot of the
    /// elaborated journal. Loading a `.dop` file is much faster than
    /// re-parsing the source, making it suitable for large ledgers that are
    /// queried repeatedly.
    Compile {
        /// Path for the output `.dop` file.
        #[arg(short, long)]
        output: PathBuf,
        /// Path to the root `.ledger` source file (may use `include`).
        source: PathBuf,
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

/// Load a [`doppio::Journal`] from either a compiled `.dop` file or a raw
/// `.ledger` source file.
///
/// The file type is detected by extension:
/// - `.dop` — decompress with XZ and deserialise with postcard.
/// - anything else — parse as Ledger source text, resolving `include`
///   directives relative to the file's parent directory.
fn load_journal(path: &PathBuf) -> Result<doppio::Journal, Box<dyn std::error::Error>> {
    if let Some("dop") = path.extension().and_then(|e| e.to_str()) {
        // Pre-compiled binary format: validate 8-byte header, then decompress
        // and deserialise.
        let mut f = File::open(path)?;
        doppio::dop_read_header(&mut f, path)?;
        // The 100 KiB scratch buffer is required by postcard's `from_io` API;
        // it does not limit the total data read.
        let input_xz = xz::read::XzDecoder::new(f);
        let buf_input = std::io::BufReader::new(input_xz);
        let mut buf = vec![0; 102400];
        Ok(postcard::from_io((buf_input, &mut buf))?.0)
    } else {
        let base_path = path.parent().unwrap().to_path_buf();
        let parser = doppio::parser::Parser {
            opener: doppio::file_opener,
            base_path,
        };
        let mut file = String::new();
        File::open(path)?.read_to_string(&mut file)?;
        Ok(doppio::compile(&file, parser)?)
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile { output, source } => {
            let base_path = source.parent().unwrap().to_path_buf();
            let parser = doppio::parser::Parser {
                opener: doppio::file_opener,
                base_path,
            };
            let mut file = String::new();
            File::open(source)?.read_to_string(&mut file)?;
            let journal = doppio::compile(&file, parser)?;
            let mut out_file = File::create(output)?;
            // Write the 8-byte header: magic (4) + version LE (2) + reserved (2).
            doppio::dop_write_header(&mut out_file)?;
            let mut output_xz = xz::write::XzEncoder::new(out_file, 1);
            {
                let mut buf = std::io::BufWriter::new(&mut output_xz);
                postcard::to_io(&journal, &mut buf)?;
                buf.flush()?;
            }
            output_xz.finish()?;
        }
        Commands::Register {
            source,
            pattern,
            begin,
            end,
            cleared,
            format,
        } => {
            let format = OutputFormat::parse(&format)?;
            let re = build_pattern_regex(pattern)?;
            let journal = load_journal(&source)?;

            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();

            let begin_date = begin
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!("invalid --begin date '{}': expected format YYYY-MM-DD", s)
                    })
                })
                .transpose()?;

            let end_date = end
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!("invalid --end date '{}': expected format YYYY-MM-DD", s)
                    })
                })
                .transpose()?;

            // Per-commodity running total across all matching postings.
            let mut running: BTreeMap<String, rust_decimal::Decimal> = BTreeMap::new();

            // Build an iterator over transactions filtered by cleared/begin/end.
            let filtered_txns: Vec<_> = journal
                .transactions
                .iter()
                .filter(|txn| {
                    if cleared
                        && !matches!(txn.state, doppio::elaboration::TransactionState::Cleared)
                    {
                        return false;
                    }
                    if begin_date.is_some() || end_date.is_some() {
                        let txn_date =
                            unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64));
                        if let Some(txn_date) = txn_date {
                            if let Some(begin) = begin_date
                                && txn_date < begin
                            {
                                return false;
                            }
                            if let Some(end) = end_date
                                && txn_date > end
                            {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();

            match format {
                OutputFormat::Text => {
                    for txn in &filtered_txns {
                        // txn.date is Unix epoch days (1970-01-01 = 0); convert back to a
                        // human-readable date string for display.
                        let date = epoch_days_to_string(txn.date);

                        for posting in txn.postings.iter() {
                            if !re.is_match(&posting.account) {
                                continue;
                            }

                            // Accumulate every commodity in this posting into the running total.
                            for (commodity, amount) in posting.amount.0.iter() {
                                *running.entry(commodity.clone()).or_default() += amount;
                            }

                            // Print one output line per commodity in the posting.
                            // The first line carries date, description, and account;
                            // subsequent commodity lines are blank in those columns.
                            let mut commodities = posting.amount.0.iter();
                            if let Some((commodity, amount)) = commodities.next() {
                                let amount_str = format!("{} {}", commodity, amount);
                                let running_str = format!(
                                    "{} {}",
                                    commodity,
                                    running.get(commodity).copied().unwrap_or_default()
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
                            for (commodity, amount) in commodities {
                                let amount_str = format!("{} {}", commodity, amount);
                                let running_str = format!(
                                    "{} {}",
                                    commodity,
                                    running.get(commodity).copied().unwrap_or_default()
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
                            if !re.is_match(&posting.account) {
                                continue;
                            }
                            for (commodity, amount) in posting.amount.0.iter() {
                                *running.entry(commodity.clone()).or_default() += amount;
                                let running_total =
                                    running.get(commodity).copied().unwrap_or_default();
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
                            if !re.is_match(&posting.account) {
                                continue;
                            }
                            for (commodity, amount) in posting.amount.0.iter() {
                                *running.entry(commodity.clone()).or_default() += amount;
                                let running_total =
                                    running.get(commodity).copied().unwrap_or_default();
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
                return Err("print only works with .ledger source files; \
                     .dop binary archives do not preserve the original transaction structure"
                    .into());
            }
            let base_path = source.parent().unwrap().to_path_buf();
            let mut parser = doppio::parser::Parser {
                opener: doppio::file_opener,
                base_path,
            };
            let mut file = String::new();
            File::open(&source)?.read_to_string(&mut file)?;
            let ast_journal: doppio::ast::Journal = parser.parse(&file)?;
            let hir: doppio::resolution::HIR = ast_journal.try_into()?;
            doppio::write_ledger(hir.transactions(), &mut std::io::stdout())?;
        }
        Commands::Accounts { source, pattern } => {
            let journal = load_journal(&source)?;
            let pattern = pattern.map(|p| p.to_lowercase()).unwrap_or_default();
            for account in journal.accounts.keys() {
                if account.to_lowercase().contains(&pattern) {
                    println!("{}", account);
                }
            }
        }
        Commands::Commodities { source } => {
            let journal = load_journal(&source)?;
            let commodities: BTreeSet<&String> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| posting.amount.0.keys())
                .collect();
            for commodity in commodities {
                println!("{}", commodity);
            }
        }
        Commands::Stats { source } => {
            let journal = load_journal(&source)?;

            let commodities: BTreeSet<&String> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| posting.amount.0.keys())
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
            depth,
            flat,
            format,
        } => {
            let format = OutputFormat::parse(&format)?;
            let re = build_pattern_regex(pattern)?;
            let journal = load_journal(&source)?;

            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();

            let begin_date = begin
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!("invalid --begin date '{}': expected format YYYY-MM-DD", s)
                    })
                })
                .transpose()?;

            let end_date = end
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!("invalid --end date '{}': expected format YYYY-MM-DD", s)
                    })
                })
                .transpose()?;

            // Balances keyed by owned account name so depth-truncation can
            // produce new strings that aren't borrowed from the journal.
            let mut balances: BTreeMap<String, BTreeMap<String, rust_decimal::Decimal>> =
                BTreeMap::new();

            for txn in journal.transactions.iter() {
                if cleared && !matches!(txn.state, doppio::elaboration::TransactionState::Cleared) {
                    continue;
                }

                if begin_date.is_some() || end_date.is_some() {
                    let txn_date =
                        unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64));
                    if let Some(txn_date) = txn_date {
                        if let Some(begin) = begin_date
                            && txn_date < begin
                        {
                            continue;
                        }
                        if let Some(end) = end_date
                            && txn_date > end
                        {
                            continue;
                        }
                    }
                }

                for posting in txn.postings.iter() {
                    if !re.is_match(&posting.account) {
                        continue;
                    }
                    let account = match depth {
                        Some(d) => truncate_account(&posting.account, d).to_owned(),
                        None => posting.account.clone(),
                    };
                    for (commodity, amount) in posting.amount.0.iter() {
                        *(balances
                            .entry(account.clone())
                            .or_default()
                            .entry(commodity.clone())
                            .or_default()) += *amount;
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

                        let mut commodities = commodities.iter();
                        if let Some((commodity, value)) = commodities.next() {
                            let balance = format!("{} {value}", commodity);
                            println!("{balance:>20}  {prefix}{label}");
                        }
                        for (commodity, value) in commodities {
                            let balance = format!("{} {value}", commodity);
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
