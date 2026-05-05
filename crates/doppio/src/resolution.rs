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

/// Higher-level Intermediate Representation (HIR) produced by the resolution stage.
///
/// The HIR holds all resolved entries (transactions, balance assertions, price
/// directives) together with the evaluation contexts needed to elaborate them.
/// It is the input to [`crate::elaborate`], which produces a fully-balanced
/// [`crate::elaboration::Journal`].
///
/// Library callers should obtain an `HIR` via [`crate::compile`] rather than
/// constructing one directly.
///
/// All entries retain their source-order position. Each [`ResolutionEntry`]
/// carries a `context_id` that indexes into [`HIR::contexts`], recording which
/// alias/default state was active for that entry.
#[derive(Debug)]
pub struct HIR {
    /// Transactions and other entries in source order.
    pub(crate) entries: Vec<ResolutionEntry>,
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
    /// Named aliases defined with `define name[(params)] = body`.
    ///
    /// During elaboration, a bare identifier or parameterized call `name(args)`
    /// in a value or boolean expression is looked up here and expanded.
    pub(crate) defines: BTreeMap<String, Define>,
}

/// A resolved `define` entry, carrying parameter names and the macro body.
#[derive(Debug, Clone)]
pub(crate) struct Define {
    /// Ordered parameter names. Empty for zero-argument defines.
    pub params: Vec<String>,
    /// The body expression to evaluate when the define is invoked.
    pub body: ast::DefineBody,
}

/// Global properties of commodities and accounts that are shared across all
/// contexts (i.e. not invalidated by later directives).
#[derive(Default, Debug)]
pub struct GlobalContext {
    /// Properties declared in `commodity` directives.
    pub commodity_properties: BTreeMap<String, CommodityProperties>,
    /// Properties declared in `account` directives.
    pub account_properties: BTreeMap<String, AccountProperties>,
    /// Properties declared in `tag` directives.
    pub tag_properties: BTreeMap<String, TagProperties>,
}

/// Validation rules for a tag declared with a `tag` directive.
#[derive(Default, Debug)]
pub struct TagProperties {
    /// Fatal assertions: elaboration halts if any fails for a matching
    /// `; TagName: value` metadata pair.
    pub(crate) asserts: Vec<ast::BoolExpr>,
    /// Non-fatal checks: a warning is printed to stderr but elaboration
    /// continues if any fails.
    pub(crate) checks: Vec<ast::BoolExpr>,
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
    /// Fatal assertions: every posting to this account must satisfy all of
    /// these expressions. Elaboration halts if any fails.
    pub(crate) asserts: Vec<ast::BoolExpr>,
    /// Non-fatal checks: if any fail, a warning is printed to stderr but
    /// elaboration continues.
    pub(crate) checks: Vec<ast::BoolExpr>,
    /// Key-value metadata declared on this account directive only — not
    /// yet inherited from ancestors. Sources include `; key: value`
    /// notes on the directive header and `key: value` sub-items inside
    /// the block. Elaboration denormalises by walking ancestors.
    pub metadata: BTreeMap<String, String>,
}

/// A single entry in the resolved journal, paired with its active context.
#[derive(Debug)]
pub(crate) struct ResolutionEntry {
    /// Index into [`HIR::contexts`]. The context at this index is the one that
    /// was active when this entry appeared in the source file.
    pub context_id: usize, // index into `Journal#contexts`
    /// The resolved entry data.
    pub data: Entry,
}

/// A resolved journal entry.
#[derive(Debug)]
pub(crate) enum Entry {
    /// A double-entry transaction with resolved dates and extracted metadata.
    Transaction(Transaction),
    /// A standalone balance assertion directive.
    Assertion(AssertionDirective),
}

/// A resolved standalone balance assertion directive.
///
/// Asserts that `account` holds `amount` on `date`. The assertion is stored
/// in the HIR for use by the elaboration stage; enforcement is a follow-up
/// (tracked in issue #37).
#[derive(Debug)]
pub(crate) struct AssertionDirective {
    /// The date at which the balance assertion applies.
    pub date: chrono::NaiveDate,
    /// The account whose balance is being asserted.
    pub account: String,
    /// The expected balance as an unevaluated expression.
    pub amount: ast::ValueExpr,
    /// `true` if `==` (strict), `false` if `=` (weak).
    pub strict: bool,
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

impl Transaction {
    /// Creates a new transaction with the given date and description.
    ///
    /// All other fields are set to their defaults: empty collections, `None`
    /// for optional fields, and [`ast::TransactionState::Uncleared`] for state.
    pub fn new(date: chrono::NaiveDate, description: impl Into<String>) -> Self {
        Self {
            date,
            description: description.into(),
            ..Default::default()
        }
    }

    /// Appends a posting to this transaction (builder pattern).
    pub fn with_posting(mut self, posting: Posting) -> Self {
        self.postings.push(posting);
        self
    }

    /// Appends a tag to this transaction (builder pattern).
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Appends a plain comment to this transaction (builder pattern).
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comments.push(comment.into());
        self
    }

    /// Inserts a metadata key-value pair into this transaction (builder pattern).
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Sets the reference code for this transaction (builder pattern).
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Sets the cleared/pending state for this transaction (builder pattern).
    pub fn with_state(mut self, state: ast::TransactionState) -> Self {
        self.state = state;
        self
    }

    /// Sets the secondary (processing) date for this transaction (builder pattern).
    pub fn with_secondary_date(mut self, date: chrono::NaiveDate) -> Self {
        self.secondary_date = Some(date);
        self
    }
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
    ///
    /// For virtual postings the surrounding markers are stripped by the parser;
    /// only the bare account name is stored here. The marker semantics live in
    /// [`Self::kind`].
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
    /// Virtual-posting kind (real, unbalanced, or balanced).
    pub kind: ast::PostingKind,
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
    /// Returns an iterator over only the [`Transaction`] entries in this HIR,
    /// skipping assertions and any other directive types.
    pub fn transactions(self) -> impl Iterator<Item = Transaction> {
        self.entries.into_iter().filter_map(|e| {
            if let Entry::Transaction(txn) = e.data {
                Some(txn)
            } else {
                None
            }
        })
    }

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
        NaiveDate::from_ymd_opt(year, ast.month, ast.date).ok_or(ResolutionError::InvalidDate)
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
                            // The `Note` arm above now handles all note values;
                            // the `Unknown("note", …)` path is superseded.
                            ast::CommodityItem::Unknown(key, value) => {
                                // Unrecognised commodity sub-key: skip with a
                                // warning rather than panicking on user input.
                                eprintln!(
                                    "warning: ignoring unrecognised commodity directive \
                                     sub-key `{key}` (value: {value:?})"
                                );
                            }
                        }
                    }
                }
                ast::Entry::Directive(ast::Directive::Account { name, notes, items }) => {
                    let global_context = result
                        .global_context
                        .account_properties
                        .entry(name.clone())
                        .or_default();

                    // Header-line and trailing `; key: value` notes contribute
                    // metadata via the same parser that handles transaction
                    // and posting notes. Bare `:tag1:tag2:` forms and free-
                    // form comments on accounts are dropped — they have no
                    // current consumer and the wire schema only carries
                    // metadata.
                    let (_tags, header_metadata, _comments) = Self::resolve_metadata(notes);
                    for (k, v) in header_metadata {
                        global_context.metadata.insert(k, v);
                    }

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
                            ast::AccountItem::Assert(expr) => {
                                global_context.asserts.push(expr);
                            }
                            ast::AccountItem::Check(expr) => {
                                global_context.checks.push(expr);
                            }
                            ast::AccountItem::Unknown(key, value) => {
                                // Sub-items without a value (e.g. a bare
                                // `; type` line) are treated as metadata
                                // with an empty value, mirroring how
                                // hledger handles the same syntax.
                                let val = value.unwrap_or_default();
                                global_context
                                    .metadata
                                    .insert(key.trim().to_string(), val.trim().to_string());
                            }
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
                ast::Entry::Directive(ast::Directive::Define { name, params, body }) => {
                    new_context = Some({
                        let mut ctx = new_context.unwrap_or_else(|| context.clone());
                        ctx.defines.insert(name, Define { params, body });
                        ctx
                    });
                }
                ast::Entry::Directive(ast::Directive::Tag {
                    name,
                    asserts,
                    checks,
                }) => {
                    let props = result
                        .global_context
                        .tag_properties
                        .entry(name)
                        .or_default();
                    for expr in asserts {
                        props.asserts.push(expr);
                    }
                    for expr in checks {
                        props.checks.push(expr);
                    }
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
                                kind: p.kind,
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
                ast::Entry::Assertion(a) => {
                    let date = Self::resolve_date(&a.date, current_default_year)?;
                    let data = Entry::Assertion(AssertionDirective {
                        date,
                        account: a.account,
                        amount: a.amount,
                        strict: a.strict,
                    });
                    result.entries.push(ResolutionEntry { context_id, data });
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
            date: ast::Date {
                year: Some(2024),
                month: 6,
                date: 15,
            },
            time: Some("14:30:00".into()),
            commodity: "AAPL".into(),
            price: ast::ValueExpr::amount(rust_decimal::Decimal::from(182), "$".into()),
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
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 15,
            },
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
        let journal = ast::Journal {
            entries: vec![ast::Entry::Transaction(txn_ast)],
        };
        let hir = HIR::try_from(journal).unwrap();

        let Entry::Transaction(ref txn) = hir.entries[0].data else {
            panic!("expected a Transaction entry");
        };
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

    #[test]
    fn test_transaction_builder() {
        use chrono::NaiveDate;

        let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        let secondary = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();

        let txn = Transaction::new(date, "Payroll")
            .with_state(ast::TransactionState::Cleared)
            .with_code("PAY-42")
            .with_secondary_date(secondary)
            .with_tag("income")
            .with_comment("monthly salary")
            .with_metadata("ref", "HR-99")
            .with_posting(
                Posting::new("Income:Salary")
                    .with_amount((rust_decimal::Decimal::from(5000u32), "USD")),
            )
            .with_posting(Posting::new("Assets:Checking"));

        assert_eq!(txn.date, date);
        assert_eq!(txn.secondary_date, Some(secondary));
        assert!(matches!(txn.state, ast::TransactionState::Cleared));
        assert_eq!(txn.code.as_deref(), Some("PAY-42"));
        assert_eq!(txn.description, "Payroll");
        assert_eq!(txn.tags, vec!["income"]);
        assert_eq!(txn.comments, vec!["monthly salary"]);
        assert_eq!(txn.metadata.get("ref").map(String::as_str), Some("HR-99"));
        assert_eq!(txn.postings.len(), 2);
        assert_eq!(txn.postings[0].account, "Income:Salary");
        assert!(txn.postings[0].amount.is_some());
        assert_eq!(txn.postings[1].account, "Assets:Checking");
    }

    #[test]
    fn test_posting_amount_from_tuple_display() {
        use rust_decimal::dec;

        let posting = Posting::new("Expenses:Food").with_amount((dec!(10.50), "$"));

        let rendered = posting.to_string();
        // The amount should appear in the rendered posting
        assert!(
            rendered.contains("10.50"),
            "expected '10.50' in: {rendered}"
        );
        assert!(rendered.contains("$"), "expected '$' in: {rendered}");
        assert!(
            rendered.contains("Expenses:Food"),
            "expected account in: {rendered}"
        );
    }

    #[test]
    fn test_define_directive_stored_in_context() {
        // A `define` directive should populate `Context::defines` and push
        // a new context version, just like other alias-modifying directives.
        let expr = ast::ValueExpr::Amount {
            value: rust_decimal::Decimal::from(1500),
            commodity: Some("$".into()),
        };
        let journal = ast::Journal {
            entries: vec![ast::Entry::Directive(ast::Directive::Define {
                name: "monthly_rent".into(),
                params: vec![],
                body: ast::DefineBody::Value(expr.clone()),
            })],
        };

        let hir = HIR::try_from(journal).unwrap();

        // A new context should have been pushed for the define directive.
        assert_eq!(hir.contexts.len(), 2);
        assert!(
            hir.contexts[1].defines.contains_key("monthly_rent"),
            "define should be stored in the new context"
        );
        // The original context must not be affected.
        assert!(hir.contexts[0].defines.is_empty());
    }

    #[test]
    fn test_define_directive_context_versioning() {
        // Transactions before a `define` see the old context; those after see
        // the context that includes the define.
        let tx_ast = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Tx".into(),
            ..Default::default()
        };
        let expr = ast::ValueExpr::Amount {
            value: rust_decimal::Decimal::from(500),
            commodity: Some("$".into()),
        };
        let journal = ast::Journal {
            entries: vec![
                ast::Entry::Transaction(tx_ast.clone()),
                ast::Entry::Directive(ast::Directive::Define {
                    name: "budget".into(),
                    params: vec![],
                    body: ast::DefineBody::Value(expr.clone()),
                }),
                ast::Entry::Transaction(tx_ast),
            ],
        };

        let hir = HIR::try_from(journal).unwrap();

        assert_eq!(
            hir.entries[0].context_id, 0,
            "tx before define should use context 0"
        );
        assert_eq!(
            hir.entries[1].context_id, 1,
            "tx after define should use context 1"
        );
        assert!(hir.contexts[1].defines.contains_key("budget"));
    }

    #[test]
    fn test_commodity_note_stored_in_global_context() {
        // Regression test for issue #91: CommodityItem::Note must be wired
        // through resolution so that note text lands in CommodityProperties,
        // not the Unknown arm that emits a spurious warning.
        let journal = ast::Journal {
            entries: vec![ast::Entry::Directive(ast::Directive::Commodity {
                name: "$".into(),
                notes: vec![],
                items: vec![
                    ast::CommodityItem::Note("American Dollars".into()),
                    ast::CommodityItem::Format("$1,000.00".into()),
                ],
            })],
        };

        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("$")
            .expect("commodity '$' should have properties");

        assert_eq!(
            props.note.as_deref(),
            Some("American Dollars"),
            "note should be stored in CommodityProperties"
        );
        assert_eq!(
            props.format.as_deref(),
            Some("$1,000.00"),
            "format should also be stored"
        );
    }

    #[test]
    fn test_assertion_directive_resolution() {
        use chrono::Datelike;

        let assertion_ast = ast::AssertionDirective {
            date: ast::Date {
                year: Some(2024),
                month: 3,
                date: 31,
            },
            account: "Assets:Checking".into(),
            amount: ast::ValueExpr::amount(rust_decimal::Decimal::from(1000), "$".into()),
            strict: true,
        };
        let journal = ast::Journal {
            entries: vec![ast::Entry::Assertion(assertion_ast)],
        };
        let hir = HIR::try_from(journal).unwrap();

        assert_eq!(hir.entries.len(), 1);
        let Entry::Assertion(ref a) = hir.entries[0].data else {
            panic!("expected Assertion entry");
        };
        assert_eq!(a.date.year(), 2024);
        assert_eq!(a.date.month(), 3);
        assert_eq!(a.date.day(), 31);
        assert_eq!(a.account, "Assets:Checking");
        assert!(a.strict);
        assert!(
            matches!(a.amount, ast::ValueExpr::Amount { commodity: Some(ref c), .. } if c == "$")
        );
    }
}
