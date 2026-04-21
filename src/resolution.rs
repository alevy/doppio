//! Resolution stage: convert an [`ast::Journal`] into the Higher-level
//! Intermediate Representation ([`HIR`]).
//!
//! This stage performs three things:
//!
//! 1. **Date resolution** — partial dates (missing year) are rejected unless
//!    a fallback year is available. Dates are converted to [`chrono::NaiveDate`].
//!
//! 2. **Alias indexing** — `commodity` and `account` directives that introduce
//!    aliases (or set a default commodity) are accumulated into a versioned
//!    [`Context`] stack. Each transaction records which context was active when
//!    it appeared in the source; the [`crate::elaboration`] stage uses this to
//!    resolve aliases at evaluation time.
//!
//! 3. **Metadata extraction** — freeform note strings attached to transactions
//!    and postings are parsed for structured data: `:tag:` syntax yields tags,
//!    and `key: value` syntax yields metadata key-value pairs.
//!
//! Amount expressions ([`ast::ValueExpr`]) are passed through untouched; they
//! are evaluated in the elaboration stage.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::ast;

/// The Higher-level Intermediate Representation produced by the resolution stage.
///
/// All entries retain their source-order position. Each [`ResolutionEntry`]
/// carries a `context_id` that indexes into [`HIR::contexts`], recording which
/// alias/default state was active for that entry.
#[derive(Debug)]
pub struct HIR {
    /// Transactions and other entries in source order.
    pub entries: Vec<ResolutionEntry>,
    /// Versioned snapshots of the alias/default-commodity state.
    ///
    /// A new [`Context`] is pushed every time a directive changes the alias
    /// table or default commodity. Entries that preceded the change continue to
    /// reference the earlier context by index — the contexts vector is
    /// append-only so old indices remain valid.
    pub contexts: Vec<Context>,
    /// Global commodity and account properties that are not context-sensitive
    /// (format strings, `nomarket` flags, notes).
    pub global_context: GlobalContext,
    /// Market price quotes collected from `P` directives in source order.
    pub prices: Vec<HistoricalPrice>,
}

/// A resolved `P` price directive.
///
/// Records the market price of one unit of `commodity` expressed as `price`
/// at the given `date` (and optionally `time`).
#[derive(Debug, Clone)]
pub struct HistoricalPrice {
    /// The date on which this price was recorded.
    pub date: NaiveDate,
    /// Optional wall-clock time of the price quote (`"HH:MM"` or `"HH:MM:SS"`).
    pub time: Option<String>,
    /// The commodity whose price is being recorded (e.g. `"AAPL"`, `"BTC"`).
    pub commodity: String,
    /// The price of one unit of `commodity` as a value expression.
    pub price: ast::ValueExpr,
}

impl Default for HIR {
    fn default() -> Self {
        Self {
            entries: vec![],
            // Start with one empty context so context_id 0 is always valid.
            contexts: vec![Context::default()],
            global_context: Default::default(),
            prices: vec![],
        }
    }
}

/// A snapshot of alias and default-commodity state at a point in the file.
///
/// Contexts form an immutable history: when a directive changes the state a
/// *new* `Context` is pushed rather than mutating the existing one. This means
/// each transaction can reference the context that was active when it was
/// defined — important because an alias that appears *after* a transaction
/// must not retroactively affect that transaction's interpretation.
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Maps short account names to their canonical equivalents.
    pub account_aliases: BTreeMap<String, String>,
    /// Maps alternative commodity symbols to their canonical names.
    pub commodity_aliases: BTreeMap<String, String>,
    /// The commodity assumed when a posting amount has no explicit commodity.
    pub default_commodity: Option<String>,
}

/// Global properties of commodities and accounts that are shared across all
/// contexts (i.e. not invalidated by later directives).
#[derive(Default, Debug)]
pub struct GlobalContext {
    /// Properties declared in `commodity` directives.
    pub commodity_properties: BTreeMap<String, CommodityProperties>,
    /// Properties declared in `account` directives.
    pub account_properties: BTreeMap<String, AccountProperties>,
}

/// Display and market-data properties of a commodity.
#[derive(Default, Debug)]
pub struct CommodityProperties {
    /// A display format string (e.g. `"1,000.00 USD"`).
    pub format: Option<String>,
    /// If `true`, this commodity is not tracked against market prices.
    pub no_market: bool,
    /// A free-form note describing the commodity.
    pub note: Option<String>,
}

/// Properties of an account declared with an `account` directive.
#[derive(Default, Debug)]
pub struct AccountProperties {
    /// A free-form note describing the account.
    pub note: Option<String>,
}

/// A single entry in the resolved journal, paired with its active context.
#[derive(Debug)]
pub struct ResolutionEntry {
    /// Index into [`HIR::contexts`]. The context at this index is the one that
    /// was active when this entry appeared in the source file.
    pub context_id: usize, // index into `Journal#contexts`
    /// The resolved entry data.
    pub data: Entry,
}

/// A resolved journal entry.
#[derive(Debug)]
pub enum Entry {
    /// A double-entry transaction with resolved dates and extracted metadata.
    Transaction(Transaction),
    /// A price directive (not yet elaborated; placeholder).
    Price(()),
    /// A balance assertion directive (not yet elaborated; placeholder).
    Assertion(()),
}

/// A transaction with fully resolved dates, tags, and metadata.
///
/// Amount expressions are still in unevaluated [`ast::AmountDetails`] form;
/// they are evaluated in the elaboration stage.
#[derive(Default, Debug)]
pub struct Transaction {
    /// The primary (effective) date, resolved to a full calendar date.
    pub date: NaiveDate,
    /// Optional secondary (processing) date.
    pub secondary_date: Option<NaiveDate>,
    /// Cleared / pending / uncleared state.
    pub state: ast::TransactionState,
    /// Optional reference code from the header.
    pub code: Option<String>,
    /// The payee / description line.
    pub description: String,
    /// Plain note lines that are neither tags nor key-value metadata.
    pub comments: Vec<String>,
    /// Tags extracted from header notes using the `:tag:` convention.
    pub tags: Vec<String>,
    /// Structured key-value metadata extracted from header notes.
    pub metadata: BTreeMap<String, String>,
    /// The postings belonging to this transaction.
    pub postings: Vec<Posting>,
}

impl std::fmt::Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.date.fmt(f)?;

        if let Some(date) = self.secondary_date {
            write!(f, "=")?;
            date.fmt(f)?;
        }

        match self.state {
            ast::TransactionState::Uncleared => {}
            ast::TransactionState::Pending => write!(f, " !")?,
            ast::TransactionState::Cleared => write!(f, " *")?,
        }

        if let Some(ref code) = self.code {
            write!(f, " ({code})")?;
        }

        if let Some((comment, &[])) = self.comments.split_first() {
            writeln!(f, " {}  ; {comment}", self.description)?;
        } else {
            writeln!(f, " {}", self.description)?;
            for comment in self.comments.iter() {
                writeln!(f, "    ; {comment}")?;
            }
        }

        for tag in self.tags.iter() {
            writeln!(f, "    ; :{tag}:")?;
        }

        for (key, value) in self.metadata.iter() {
            writeln!(f, "    ; {key}: {value}")?;
        }

        for posting in self.postings.iter() {
            posting.fmt(f)?;
        }

        Ok(())
    }
}

/// A posting with extracted tags and metadata.
///
/// The `amount` field is still an unevaluated [`ast::AmountDetails`] tree.
#[derive(Default, Debug)]
pub struct Posting {
    /// The account name as written in the source (not yet alias-resolved).
    pub account: String,
    /// The unevaluated amount, or `None` for a null posting.
    pub amount: Option<ast::AmountDetails>,
    /// Per-posting state.
    pub state: ast::TransactionState,
    /// Tags extracted from posting notes.
    pub tags: Vec<String>,
    /// Key-value metadata extracted from posting notes.
    pub metadata: BTreeMap<String, String>,
    /// Plain note lines that are neither tags nor key-value metadata.
    pub comments: Vec<String>,
}

impl Posting {
    /// Creates a new posting for `account` with no amount, tags, or metadata.
    pub fn new<S: Into<String>>(account: S) -> Self {
        Self {
            account: account.into(),
            ..Default::default()
        }
    }

    /// Appends a tag to this posting (builder pattern).
    pub fn with_tag<S: Into<String>>(mut self, tag: S) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Appends a plain comment to this posting (builder pattern).
    pub fn with_comment<S: Into<String>>(mut self, comment: S) -> Self {
        self.comments.push(comment.into());
        self
    }

    /// Inserts a metadata key-value pair into this posting (builder pattern).
    pub fn with_metadata<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Sets the amount for this posting (builder pattern).
    pub fn with_amount<A: Into<ast::AmountDetails>>(mut self, amount: A) -> Self {
        self.amount = Some(amount.into());
        self
    }
}

impl std::fmt::Display for Posting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "    ")?;
        match self.state {
            ast::TransactionState::Uncleared => {}
            ast::TransactionState::Pending => write!(f, "! ")?,
            ast::TransactionState::Cleared => write!(f, "* ")?,
        }

        write!(f, "{}", self.account)?;

        if let Some(ref amount) = self.amount {
            write!(f, "  {amount}")?;
        }
        if let Some((comment, &[])) = self.comments.split_first() {
            writeln!(f, "  ; {comment}")?;
        } else {
            writeln!(f)?;
            for comment in self.comments.iter() {
                writeln!(f, "    ; {comment}")?;
            }
        }

        for tag in self.tags.iter() {
            writeln!(f, "    ; :{tag}:")?;
        }

        for (key, value) in self.metadata.iter() {
            writeln!(f, "    ; {key}: {value}")?;
        }
        Ok(())
    }
}

/// Errors that can occur during the resolution stage.
#[derive(Debug)]
pub enum ResolutionError {
    /// A date could not be resolved: either the year was absent and no
    /// fallback was available, or the resulting calendar date is invalid
    /// (e.g. February 30).
    InvalidDate,
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionError::InvalidDate => {
                write!(f, "Invalid date")
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

impl HIR {
    /// Resolve an [`ast::Date`] to a [`NaiveDate`].
    ///
    /// If `ast.year` is `None`, `fallback_year` is used instead. Returns
    /// `Err(ResolutionError::InvalidDate)` if no year is available or if the
    /// resulting date does not exist in the calendar (e.g. Feb 30).
    fn resolve_date(
        ast: &ast::Date,
        fallback_year: Option<i32>,
    ) -> Result<NaiveDate, ResolutionError> {
        let year = ast
            .year
            .or(fallback_year)
            .ok_or(ResolutionError::InvalidDate)?;
        NaiveDate::from_ymd_opt(year, ast.month, ast.date)
            .ok_or(ResolutionError::InvalidDate)
    }

    /// Parse tags and key-value metadata out of a list of note strings.
    ///
    /// Ledger supports two structured note conventions:
    ///
    /// - **Tags**: a note of the form `:tag1:tag2:` (colon-enclosed, colon-
    ///   separated) produces individual tag strings `["tag1", "tag2"]`.
    /// - **Metadata**: a note of the form `key: value` produces a key-value
    ///   pair `("key", "value")`.
    ///
    /// Notes that match neither pattern are preserved as plain comments in the
    /// third element of the returned tuple.
    fn resolve_metadata(
        notes: Vec<String>,
    ) -> (Vec<String>, BTreeMap<String, String>, Vec<String>) {
        let mut tags: Vec<String> = vec![];
        let mut metadata: BTreeMap<String, String> = Default::default();
        let mut comments: Vec<String> = vec![];

        for note in notes {
            let note = note.trim();
            if let Some(note) = note.strip_prefix(":")
                && let Some(note) = note.strip_suffix(":")
            {
                // ":tag1:tag2:" — split on ":" to get individual tags
                for tag in note.split(":") {
                    tags.push(tag.into());
                }
            } else if let Some((key, value)) = note.split_once(":") {
                // "key: value" — insert as metadata
                metadata.insert(key.trim().into(), value.trim().into());
            } else {
                // Plain comment — preserve rather than discard
                comments.push(note.to_string());
            }
        }
        (tags, metadata, comments)
    }
}

impl TryFrom<ast::Journal> for HIR {
    type Error = ResolutionError;

    fn try_from(ast: ast::Journal) -> Result<Self, Self::Error> {
        let mut result: HIR = Default::default();

        #[allow(unused_mut)]
        let mut current_default_year = None;

        for entry in ast.entries {
            // `new_context` accumulates changes from directives in this entry.
            // If any directive modifies the context, a new Context is pushed at
            // the end of the loop iteration, and subsequent entries reference it.
            let mut new_context: Option<Context> = None;

            // The current context is whichever was most recently pushed.
            let context_id = result.contexts.len() - 1; // always at least zero;
            let context = &result.contexts[context_id];

            match entry {
                ast::Entry::Directive(ast::Directive::Unknown(_)) | ast::Entry::Comment(_) => {
                    // Discard unrecognised directives and comments.
                }
                ast::Entry::Directive(ast::Directive::Commodity {
                    name,
                    notes: _,
                    items,
                }) => {
                    let global_context = result
                        .global_context
                        .commodity_properties
                        .entry(name.clone())
                        .or_default();
                    for item in items {
                        match item {
                            ast::CommodityItem::Alias(alias) => {
                                // Clone the current context before mutating so
                                // entries that precede this directive keep their
                                // original view of aliases.
                                new_context = Some(new_context.unwrap_or_else(|| context.clone()))
                                    .map(|mut ctx| {
                                        ctx.commodity_aliases.insert(alias, name.clone());
                                        ctx
                                    });
                            }
                            ast::CommodityItem::Default => {
                                new_context = Some(new_context.unwrap_or_else(|| context.clone()))
                                    .map(|mut ctx| {
                                        ctx.default_commodity = Some(name.clone());
                                        ctx
                                    });
                            }
                            ast::CommodityItem::Format(format) => {
                                global_context.format = Some(format);
                            }
                            ast::CommodityItem::NoMarket => {
                                global_context.no_market = true;
                            }
                            ast::CommodityItem::Note(note) => {
                                global_context.note = Some(note);
                            }
                            ast::CommodityItem::Unknown(key, Some(value)) if &key == "note" => {
                                global_context.note = Some(value);
                            }
                            ast::CommodityItem::Unknown(key, value) => todo!("{key} {value:?}"),
                        }
                    }
                }
                ast::Entry::Directive(ast::Directive::Account {
                    name,
                    notes: _,
                    items,
                }) => {
                    let global_context = result
                        .global_context
                        .account_properties
                        .entry(name.clone())
                        .or_default();

                    for item in items {
                        match item {
                            ast::AccountItem::Alias(alias) => {
                                new_context = Some(new_context.unwrap_or_else(|| context.clone()))
                                    .map(|mut ctx| {
                                        ctx.account_aliases.insert(alias, name.clone());
                                        ctx
                                    });
                            }
                            ast::AccountItem::Note(note) => global_context.note = Some(note),
                            ast::AccountItem::Unknown(_, _) => { /* TODO */ }
                        }
                    }
                }
                ast::Entry::Directive(ast::Directive::Alias { alias, account }) => {
                    new_context = Some({
                        let mut ctx = context.clone();
                        ctx.account_aliases.insert(alias, account);
                        ctx
                    });
                }
                ast::Entry::Transaction(transaction) => {
                    let date = Self::resolve_date(&transaction.date, current_default_year)?;
                    let secondary_date = if let Some(ref d) = transaction.secondary_date {
                        Some(Self::resolve_date(d, current_default_year)?)
                    } else {
                        None
                    };

                    let (tags, metadata, comments) = Self::resolve_metadata(transaction.notes);
                    let postings = transaction
                        .postings
                        .into_iter()
                        .map(|p| {
                            let (tags, metadata, comments) = Self::resolve_metadata(p.notes);

                            Posting {
                                account: p.account,
                                amount: p.amount,
                                state: p.state,
                                tags,
                                metadata,
                                comments,
                            }
                        })
                        .collect();

                    let data = Entry::Transaction(Transaction {
                        date,
                        secondary_date,
                        state: transaction.state,
                        code: transaction.code,
                        description: transaction.description,
                        comments,
                        tags,
                        metadata,
                        postings,
                    });

                    result.entries.push(ResolutionEntry { context_id, data });
                }
                ast::Entry::HistoricalPrice(hp) => {
                    let date = Self::resolve_date(&hp.date, current_default_year)?;
                    result.prices.push(HistoricalPrice {
                        date,
                        time: hp.time,
                        commodity: hp.commodity,
                        price: hp.price,
                    });
                }
            }

            // If any directive modified the alias/default state, push a new
            // context version so subsequent entries see the updated aliases.
            if let Some(new_context) = new_context {
                result.contexts.push(new_context);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod resolution_tests {
    use chrono::Datelike;

    use super::*;
    use crate::ast;

    #[test]
    fn test_date_resolution() {
        // Case: Successful full date
        let d1 = ast::Date {
            year: Some(2024),
            month: 2,
            date: 29,
        };
        assert!(HIR::resolve_date(&d1, None).is_ok());

        // Case: Fallback year logic
        let d2 = ast::Date {
            year: None,
            month: 1,
            date: 15,
        };
        let resolved = HIR::resolve_date(&d2, Some(2023)).unwrap();
        assert_eq!(resolved.year(), 2023);

        // Case: No year available (Error)
        assert!(matches!(
            HIR::resolve_date(&d2, None),
            Err(ResolutionError::InvalidDate)
        ));

        // Case: Calendar invalidity (Feb 30)
        let d3 = ast::Date {
            year: Some(2023),
            month: 2,
            date: 30,
        };
        assert!(matches!(
            HIR::resolve_date(&d3, None),
            Err(ResolutionError::InvalidDate)
        ));
    }

    #[test]
    fn test_metadata_extraction() {
        let notes = vec![
            ":Financial:Tax:".to_string(),
            "  Invoice: 1234  ".to_string(),
            "Random comment".to_string(),
        ];
        let (tags, meta, comments) = HIR::resolve_metadata(notes);

        assert_eq!(tags, vec!["Financial", "Tax"]);
        assert_eq!(meta.get("Invoice").unwrap(), "1234");
        assert_eq!(meta.len(), 1);
        assert_eq!(comments, vec!["Random comment"]);
    }

    #[test]
    fn test_context_versioning() {
        let mut journal = ast::Journal { entries: vec![] };

        // Setup: Transaction -> Alias Directive -> Transaction
        // We want to ensure Tx1 uses Context 0 and Tx2 uses Context 1.

        let tx_ast = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Tx".into(),
            ..Default::default()
        };

        journal
            .entries
            .push(ast::Entry::Transaction(tx_ast.clone()));
        journal
            .entries
            .push(ast::Entry::Directive(ast::Directive::Commodity {
                name: "BTC".into(),
                notes: vec![],
                items: vec![ast::CommodityItem::Alias("Bitcoin".into())],
            }));
        journal.entries.push(ast::Entry::Transaction(tx_ast));

        let hir = HIR::try_from(journal).unwrap();

        assert_eq!(hir.contexts.len(), 2);
        assert_eq!(hir.entries[0].context_id, 0);
        assert_eq!(hir.entries[1].context_id, 1);

        // Verify context 1 has the alias
        assert_eq!(
            hir.contexts[1].commodity_aliases.get("Bitcoin").unwrap(),
            "BTC"
        );
        // Verify context 0 does not
        assert!(hir.contexts[0].commodity_aliases.is_empty());
    }

    #[test]
    fn test_historical_price_resolution() {
        use chrono::Datelike;
        let price_ast = ast::HistoricalPrice {
            date: ast::Date { year: Some(2024), month: 6, date: 15 },
            time: Some("14:30:00".into()),
            commodity: "AAPL".into(),
            price: ast::ValueExpr::amount(
                rust_decimal::Decimal::from(182),
                "$".into(),
            ),
        };
        let journal = ast::Journal {
            entries: vec![ast::Entry::HistoricalPrice(price_ast)],
        };
        let hir = HIR::try_from(journal).unwrap();

        assert_eq!(hir.prices.len(), 1);
        let price = &hir.prices[0];
        assert_eq!(price.date.year(), 2024);
        assert_eq!(price.date.month(), 6);
        assert_eq!(price.date.day(), 15);
        assert_eq!(price.time.as_deref(), Some("14:30:00"));
        assert_eq!(price.commodity, "AAPL");
    }

    #[test]
    fn test_comment_preservation_roundtrip() {
        // Build an AST transaction with mixed note types and verify that
        // after resolution the comments, tags, and metadata are separated.
        let txn_ast = ast::Transaction {
            date: ast::Date { year: Some(2024), month: 1, date: 15 },
            description: "Groceries".into(),
            notes: vec![
                "just a note".into(),
                "Invoice: 42".into(),
                ":groceries:".into(),
            ],
            postings: vec![
                ast::Posting::new("Expenses:Food")
                    .with_note("posting note")
                    .with_amount((rust_decimal::Decimal::TEN, "$")),
                ast::Posting::new("Assets:Checking"),
            ],
            ..Default::default()
        };
        let journal = ast::Journal { entries: vec![ast::Entry::Transaction(txn_ast)] };
        let hir = HIR::try_from(journal).unwrap();

        let Entry::Transaction(ref txn) = hir.entries[0].data else { panic!() };
        assert_eq!(txn.comments, vec!["just a note"]);
        assert_eq!(txn.metadata.get("Invoice").unwrap(), "42");
        assert_eq!(txn.tags, vec!["groceries"]);
        assert_eq!(txn.postings[0].comments, vec!["posting note"]);
    }

    #[test]
    fn test_posting_builder() {
        let posting = Posting::new("Expenses:Food")
            .with_tag("groceries")
            .with_comment("weekly shop")
            .with_metadata("ref", "123");

        assert_eq!(posting.account, "Expenses:Food");
        assert_eq!(posting.tags, vec!["groceries"]);
        assert_eq!(posting.comments, vec!["weekly shop"]);
        assert_eq!(posting.metadata.get("ref").unwrap(), "123");
        assert!(posting.amount.is_none());
    }

    #[test]
    fn test_transaction_display_with_comment() {
        use chrono::NaiveDate;
        let txn = Transaction {
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            description: "Groceries".into(),
            comments: vec!["weekly shop".into()],
            postings: vec![Posting::new("Expenses:Food")],
            ..Default::default()
        };
        let s = txn.to_string();
        assert!(s.contains("Groceries  ; weekly shop"));
        assert!(s.contains("Expenses:Food"));
    }
}
