//! Resolution stage: convert an [`ast::Journal`] into the Higher-level
//! Intermediate Representation ([`HIR`]).
//!
//! This stage performs three things:
//!
//! 1. **Date resolution** -- partial dates (missing year) are rejected unless
//!    a fallback year is available. Dates are converted to [`chrono::NaiveDate`].
//!
//! 2. **Alias indexing** -- `commodity` and `account` directives that introduce
//!    aliases (or set a default commodity) are accumulated into a versioned
//!    [`Context`] stack. Each transaction records which context was active when
//!    it appeared in the source; the [`crate::elaboration`] stage uses this to
//!    resolve aliases at evaluation time.
//!
//! 3. **Metadata extraction** -- freeform note strings attached to transactions
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
    /// reference the earlier context by index -- the contexts vector is
    /// append-only so old indices remain valid.
    pub contexts: Vec<Context>,
    /// Global commodity and account properties that are not context-sensitive
    /// (format strings, `nomarket` flags, notes).
    pub global_context: GlobalContext,
    /// Market price quotes collected from `P` directives in source order.
    pub prices: Vec<HistoricalPrice>,
    /// Automated posting rules (`= QUERY\n POSTINGS…`) collected during
    /// resolution, in source order. Applied by the elaborator to every
    /// real transaction: for each posting whose account name matches a
    /// rule's query, the rule's body postings are synthesised as
    /// virtual-unbalanced entries appended to that transaction.
    pub(crate) auto_rules: Vec<ResolvedAutoRule>,
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
            auto_rules: vec![],
        }
    }
}

/// A body posting within a resolved automated posting rule.
///
/// `amount` is `None` when the source posting was a null posting (no amount
/// written). `multiplier` carries a commodity-less decimal that acts as a
/// scale factor applied to the matched posting's amount; it is `None` when
/// the body amount carried an explicit commodity (literal).
///
/// The `kind` from the source is intentionally not carried: all synthesised
/// postings are forced to `VirtualUnbalanced` regardless of what was written
/// in the rule body (this matches ledger-cli convention for `=` rules).
#[derive(Debug, Clone)]
pub(crate) struct ResolvedAutoRulePosting {
    /// The account name for the synthesised posting (not yet alias-resolved).
    pub account: String,
    /// The raw amount details if an amount was written.
    pub amount: Option<ast::AmountDetails>,
}

/// A resolved automated posting rule.
///
/// `query` is the compiled regex against which each real transaction's
/// posting accounts are tested; on a match, every body posting in `postings`
/// is instantiated as a virtual-unbalanced synthesised posting appended to
/// that transaction.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedAutoRule {
    /// Compiled query pattern. `/pattern/` queries become the bare regex;
    /// bare-string queries become a case-insensitive substring regex.
    pub query: regex::Regex,
    /// The body postings to instantiate on a match.
    pub postings: Vec<ResolvedAutoRulePosting>,
}

/// A snapshot of alias and default-commodity state at a point in the file.
///
/// Contexts form an immutable history: when a directive changes the state a
/// *new* `Context` is pushed rather than mutating the existing one. This means
/// each transaction can reference the context that was active when it was
/// defined -- important because an alias that appears *after* a transaction
/// must not retroactively affect that transaction's interpretation.
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Maps short account names to their canonical equivalents.
    pub account_aliases: BTreeMap<String, String>,
    /// Maps alternative commodity symbols to their canonical names and conversion divisors.
    ///
    /// The key is the alias (source) commodity symbol. The value is
    /// `(canonical_symbol, divisor)` where `divisor` is the amount to divide by
    /// when converting to the canonical commodity. Aliases populated from
    /// `commodity X\n  alias Y` directives use `divisor = Decimal::ONE` (a 1:1
    /// rename). Full `C` conversion directives (stage 2, #248) will populate
    /// entries with arbitrary divisors.
    pub commodity_conversions: BTreeMap<String, (String, rust_decimal::Decimal)>,
    /// The commodity assumed when a posting amount has no explicit commodity.
    pub default_commodity: Option<String>,
    /// Named aliases defined with `define name[(params)] = body`.
    ///
    /// During elaboration, a bare identifier or parameterized call `name(args)`
    /// in a value or boolean expression is looked up here and expanded.
    pub(crate) defines: BTreeMap<String, Define>,
}

impl Context {
    /// Run a fixpoint pass over `commodity_conversions`, transitively resolving
    /// every entry to its chain root.
    ///
    /// After this pass each entry `(Y, (root, total_divisor))` satisfies:
    /// `root` either is not itself a key in `commodity_conversions` (true chain
    /// root), or is a self-loop sentinel `root == root` (canonical commodity
    /// that is already expressed in itself, with a divisor for unit normalisation).
    /// The divisor accumulates multiplicatively along the chain so that
    /// `amount_in_Y / total_divisor == amount_in_root`.
    ///
    /// Cycles (`C 1X = 1Y; C 1Y = 1X`) are detected via a hop-count limit equal
    /// to the map's length: if we take more hops than there are entries, we must
    /// be in a cycle. The commodity that triggered the cycle is returned as an
    /// error.
    ///
    /// This is called once per `CommodityConversion` directive (eager/resolution-time
    /// fixpoint, as required by #266 design constraint A).
    pub(crate) fn resolve_commodity_conversion_chains(&mut self) -> Result<(), ResolutionError> {
        let max_hops = self.commodity_conversions.len();
        // Collect all commodity keys first to avoid borrow conflicts.
        let keys: Vec<String> = self.commodity_conversions.keys().cloned().collect();
        for start in keys {
            // Walk the chain from `start` until we reach a root.
            //
            // A root is either:
            //   (a) a commodity that is NOT a key in the map (truly external root), or
            //   (b) a self-loop sentinel: (X, (X, d)) — canonical commodity X maps to
            //       itself with normalisation divisor d.
            //
            // In either case we stop and accumulate the final divisor.
            let mut hops = 0usize;
            let mut current = start.clone();
            let mut total_divisor = rust_decimal::Decimal::ONE;

            // Walk the chain until we hit a root (commodity not in the map)
            // or a self-loop sentinel (commodity maps to itself).
            //
            // Self-loop sentinels always carry divisor = 1 (since #274 changed
            // the LHS insertion to use `Decimal::ONE`). When we hit a self-loop
            // we stop WITHOUT multiplying its divisor into the total — multiplying
            // by 1 is a no-op anyway, but the explicit break-before-multiply
            // documents the intent: the self-loop is a terminus marker, not a
            // scaling step.
            while let Some((next, divisor)) = self.commodity_conversions.get(&current).cloned() {
                if next == current {
                    // Self-loop sentinel: this is the chain root; stop.
                    break;
                }
                total_divisor *= divisor;
                current = next;
                hops += 1;
                if hops > max_hops {
                    return Err(ResolutionError::CommodityConversionCycle(start));
                }
            }

            // Rewrite the entry for `start` to point directly at the chain root
            // with the accumulated total divisor.
            if let Some(entry) = self.commodity_conversions.get_mut(&start) {
                *entry = (current, total_divisor);
            }
        }
        Ok(())
    }
}

/// A resolved `define` entry, carrying parameter names and the macro body.
#[derive(Debug, Clone)]
pub(crate) struct Define {
    /// Ordered parameter names. Empty for zero-argument defines.
    pub params: Vec<String>,
    /// The body expression to evaluate when the define is invoked.
    pub body: ast::DefineBody,
}

/// Per-journal state populated during resolution.
///
/// Holds journal-derived metadata (commodity / account / tag properties,
/// per-commodity tolerance overrides set by `option` directives) — i.e.
/// things the resolver builds from directives in the source file. The
/// elaborator's *semantic configuration* (default tolerance rule, balance
/// mode, assertion scope, ...) is **not** here -- that lives in
/// [`ElaborationConfig`] and is passed explicitly to `elaborate()`. The
/// two layers are deliberately separate: a frontend's job is to parse a
/// dialect into the AST, the resolver canonicalises the AST into the
/// HIR, and the elaborator applies a semantic ruleset that the caller
/// chooses (typically the matching tool's defaults, but freely mixable).
#[derive(Default, Debug)]
pub struct GlobalContext {
    /// Properties declared in `commodity` directives.
    pub commodity_properties: BTreeMap<String, CommodityProperties>,
    /// Properties declared in `account` directives.
    pub account_properties: BTreeMap<String, AccountProperties>,
    /// Properties declared in `tag` directives.
    pub tag_properties: BTreeMap<String, TagProperties>,
    /// Per-commodity absolute tolerance overrides populated by Beancount's
    /// `option "inferred_tolerance_default" "COMMODITY:VALUE"` directive.
    /// When a commodity is present in this map, the elaborator uses the
    /// override directly; otherwise it falls back to
    /// [`ElaborationConfig::tolerance_mode`]. This map is journal state
    /// (mutated by directives during resolution), not config -- and is
    /// therefore the one tolerance-related field that stays on
    /// `GlobalContext`.
    pub tolerance_overrides: BTreeMap<String, rust_decimal::Decimal>,
}

/// The elaborator's semantic configuration.
///
/// Passed explicitly to [`crate::elaborate`] alongside the [`HIR`].
/// Keeps elaboration semantics decoupled from the frontend that parsed
/// the source: a journal in beancount syntax can be elaborated under
/// ledger-cli rules, hledger syntax under beancount rules, or any
/// mix-and-match combination. The expected common case --
/// "use the same tool's defaults that produced this dialect" -- is
/// served by the [`crate::frontend::Frontend::defaults`] convenience
/// per-frontend default constants.
///
/// # Layering with journal-injected overrides
///
/// Journal directives can override individual fields per commodity /
/// account: the most prominent example is Beancount's
/// `option "inferred_tolerance_default" "COMMODITY:VALUE"` which
/// populates [`GlobalContext::tolerance_overrides`]. The elaborator
/// reads the override map first and falls back to the config's
/// [`Self::tolerance_mode`] when no override is present. The config
/// stays immutable per elaboration call; the override map is
/// journal-derived and lives on `GlobalContext`.
///
/// Marked `#[non_exhaustive]`: external callers can construct via
/// `Default::default()` or one of the per-frontend defaults
/// (`ledger_defaults()`, `hledger_defaults()`, `beancount_defaults()`)
/// and mutate fields, but cannot use struct-literal syntax with
/// `..Default::default()` from outside the crate. New per-frontend
/// semantic axes are expected (e.g. `LotValidationMode` and
/// `default_booking_method` were both added post-1.0.0); the
/// non-exhaustive marker keeps further additions additive rather
/// than major-breaking.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ElaborationConfig {
    /// Default per-transaction balance-residual tolerance. Per-commodity
    /// overrides on [`GlobalContext::tolerance_overrides`] take
    /// precedence; this rule applies to commodities not present in the
    /// override map. See [`ToleranceMode`].
    pub tolerance_mode: ToleranceMode,
    /// How the elaborator computes a posting's cash-equivalent for
    /// transaction balance when the posting carries both `{cost}` and
    /// `@price` annotations. See [`BalanceMode`].
    pub balance_mode: BalanceMode,
    /// Whether top-level `balance` directives ([`Entry::Assertion`])
    /// check the named account in isolation or aggregate the entire
    /// account subtree. See [`AssertionScope`].
    pub assertion_scope: AssertionScope,
    /// Whether the elaborator validates that a posting bearing a lot
    /// annotation matches an existing position when the posting reduces
    /// the account's running balance for that commodity. See
    /// [`LotValidationMode`].
    pub lot_validation_mode: LotValidationMode,
    /// Frontend-default booking method, used when an account has no
    /// explicit `booking_method` of its own. Beancount applies STRICT
    /// here (matching `option "booking_method" "STRICT"` and the
    /// implicit default on `open` directives); ledger-cli / hledger
    /// have no booking concept and use NONE (the booking pass is a
    /// no-op for those frontends, even when a posting carries a
    /// cost-MISSING lot annotation like `[date]` only).
    pub default_booking_method: BookingMethod,
    /// When `true`, the elaborator synthesises an implicit `@@` (total-cost)
    /// annotation on the non-cash leg of a two-real-posting multi-commodity
    /// transaction when neither posting carries an explicit cost (`{}`),
    /// `@`/`@@` price annotation. This matches ledger-cli and hledger
    /// semantics: the cash leg's absolute amount becomes the inferred total
    /// cost basis of the non-cash leg, and the transaction is treated as
    /// balanced.
    ///
    /// Set `true` for ledger-cli and hledger frontends, `false` (the
    /// default) for Beancount, which requires an explicit cost on every
    /// lot-bearing posting.
    pub infer_implicit_total_cost: bool,
}

/// Strategy for computing a posting's cash-equivalent during the
/// transaction balance check when the posting carries both a lot
/// `{cost}` annotation and an `@price` annotation.
///
/// Surfaced from #210, which observed that ledger-cli + Beancount use
/// `{cost}` for balance (with `@price` informational), while hledger
/// uses `@price` (with `{cost}` informational). doppio's prior
/// behaviour matched hledger; the other two frontends produced
/// `TransactionDoesNotBalance` whenever the user wrote an explicit
/// PnL posting alongside `{cost} @price`.
#[derive(Debug, Clone, Default)]
pub enum BalanceMode {
    /// `{cost}` drives the cash-equivalent for balance; `@price` is
    /// informational only. The user is expected to write an explicit
    /// gain/loss posting that absorbs any cost-vs-price residual.
    /// Default. Matches ledger-cli and Beancount.
    #[default]
    CostBasis,
    /// `@price` (when present) drives the cash-equivalent for
    /// balance; `{cost}` is informational. The user does NOT write a
    /// gain/loss posting; doppio synthesizes one on `gains_account`
    /// after the @price-driven balance succeeds, computed as
    /// `units * (cost - price)`, so the elaborated transaction is
    /// also cost-basis-balanced (and the `.dop` output is uniform
    /// with what Beancount/ledger-cli inputs produce). Matches
    /// hledger.
    AtPriceWithSynthesis { gains_account: String },
}

/// Scope of a top-level `balance` directive's account lookup.
///
/// Three frontends, two semantics:
///
/// - **Beancount** (`balance Account X CUR`): the running balance is
///   summed across `Account` itself and every descendant whose name
///   has `Account + ":"` as a prefix. A `pad` that targets the same
///   account computes its corrective amount from the same subtree
///   sum.
/// - **ledger-cli / hledger** (`Account = X CUR` posting assertion or
///   the doppio-extended top-level form): the running balance is the
///   direct posting balance of the named account only. hledger's
///   strict `==` form is explicit about this in its own error message
///   ("excluding subaccounts"). hledger's `==*` posting form is
///   subtree-aware, but it is encoded as a separate
///   [`crate::ast::AmountDetails::BalanceAssignmentAllCommodities`]
///   variant rather than via this scope flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AssertionScope {
    /// The named account's running balance only. Default.
    #[default]
    Direct,
    /// Sum of the named account and every descendant.
    Subtree,
}

/// Strategy for how the elaborator interprets lot annotations on
/// reducing postings.
///
/// All three reference tools track a per-(commodity, lot) inventory; the
/// difference is whether they validate that a reducing posting names a
/// lot that actually exists in the inventory:
///
/// - **ledger-cli / hledger** treat the lot annotation as a label
///   carried alongside the posting. A reducing posting may name any lot
///   key, even one with no prior augmentation. The lot dimension is
///   recorded but never enforced.
/// - **Beancount** rejects a reducing posting whose lot key has no
///   matching prior augmentation in the same account+commodity. This is
///   the precondition that makes lot-aware reports (per-lot capital
///   gains, FIFO/LIFO booking) sound: every reduction can be traced to
///   a specific augmentation.
///
/// Surfaced from #237.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LotValidationMode {
    /// Lot annotations are recorded on postings and threaded into the
    /// running inventory, but no validation is performed when a
    /// reducing posting names a lot. Matches ledger-cli and hledger.
    /// Default.
    #[default]
    Permissive,
    /// A reducing posting (one whose quantity has the opposite sign of
    /// the existing position in that account+commodity) bearing a lot
    /// annotation must name a lot key already present in the
    /// inventory. Phantom-lot reductions raise
    /// [`crate::elaborator::ElaborationError::PhantomLotReduction`].
    /// Matches Beancount's strict booking method.
    Strict,
}

/// Booking method declared on a Beancount `open` directive — governs
/// how an ambiguous `{}` (empty cost spec) reduction is resolved
/// against the account's running inventory.
///
/// Booking only fires when the user writes `{}` (or a partial spec
/// like `{2024-01-15}` that leaves the cost MISSING). A bare
/// reduction with no `{}` at all is not booked: it is recorded as a
/// `cost=None` position and the booking method is not consulted
/// (matching Beancount 3.x — see [`LotValidationMode`] for the
/// validation orthogonal to this).
///
/// The variants mirror Beancount's `Booking` enum (in
/// `beancount/core/data.py`) modulo casing and naming. Marked
/// `#[non_exhaustive]` because Beancount itself has added booking
/// methods over its history; doppio expects to track those.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BookingMethod {
    /// Reject ambiguous `{}` matches with an error. Default in
    /// Beancount; the only resolution allowed is when exactly one
    /// existing lot satisfies the partial spec, or when the reduction
    /// would empty the account in that commodity.
    #[default]
    Strict,
    /// Like [`Self::Strict`], but if a single lot matches the
    /// reduction's *size* exactly, that lot wins as if it were
    /// unambiguous.
    StrictWithSize,
    /// No matching at all: the reduction is appended to the inventory
    /// as a cost=None position even if it would result in a mixed
    /// inventory.
    None,
    /// Collapse all lots in the same commodity into a single
    /// weighted-average-cost lot before matching.
    Average,
    /// First-in first-out: consume oldest lots first.
    Fifo,
    /// Last-in first-out: consume most-recent lots first.
    Lifo,
    /// Highest-in first-out: consume the most-expensive lot first.
    Hifo,
}

/// Per-transaction balance tolerance policy.
///
/// When a transaction's postings sum to a non-zero residual, the
/// elaborator either rejects the transaction (residual exceeds
/// tolerance) or absorbs the residual into a synthesized posting
/// whose account is the empty string `""` -- the doppio convention
/// for "rounding residual; not a user-named account."
///
/// Marked `#[non_exhaustive]`: alternate tolerance models (e.g.
/// fixed-cents, per-commodity-explicit) are plausible additions
/// and shouldn't trigger a major SemVer break.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum ToleranceMode {
    /// Accept residuals whose absolute value is at most
    /// `fraction * 10^(-min_scale)` per commodity.
    ///
    /// `min_scale` is resolved with priority:
    ///
    /// 1. **`inferred_scale` from [`CommodityProperties`]** — derived from
    ///    `D`/`commodity format` directives and direct posting scales (#281).
    ///    When present (and non-zero), this embodies the journal's declared
    ///    precision intent. A residue smaller than `10^(-inferred_scale)` is
    ///    sub-precision noise, consistent with ledger-cli's commodity
    ///    display-precision `is_zero()` check (xact.cc:872-904).
    ///
    /// 2. **Least-precise resolved posting's decimal scale** — used as
    ///    fallback when the commodity has no `CommodityProperties` entry
    ///    (the commodity appeared in no direct posting and has no
    ///    `D`/`commodity format` directive). This is Beancount's original
    ///    inferred-tolerance rule.
    ///
    /// Common fraction values:
    ///
    /// - `fraction = 0`: every transaction must balance to exact zero per
    ///   commodity.
    /// - `fraction = 0.5`: accept residuals up to half the least-precise
    ///   posting's decimal place. Matches Beancount's default (e.g. for
    ///   scale-2 USD postings, tolerance = 0.005).
    /// - `fraction = 1`: accept residuals up to one full unit at the
    ///   commodity's declared precision. Combined with inferred_scale, this
    ///   matches ledger-cli and hledger behaviour for journals with
    ///   28-decimal IEEE-double `@`-prices and an explicit dust-compensation
    ///   leg (the standard.dat:4244 pattern, #286).
    FractionOfSmallestPrecision(rust_decimal::Decimal),
}

impl Default for ToleranceMode {
    fn default() -> Self {
        Self::FractionOfSmallestPrecision(rust_decimal::Decimal::ZERO)
    }
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
    /// Maximum decimal scale intended for this commodity, inferred from the
    /// journal source. Populated during resolution as a side-effect of the
    /// AST → HIR walk. Used by the elaborator to round `qty * price` costs
    /// for `@`-priced postings to the journal's intended precision before the
    /// balance check, matching ledger-cli's commodity display-precision
    /// mechanism (xact.cc:872-904). Defaults to 0 when no postings of this
    /// commodity were seen.
    ///
    /// # Scale-source priority
    ///
    /// Two authoritative sources contribute to this value; the maximum across
    /// both is taken:
    ///
    /// 1. **`D` / `commodity format` directives** — the format string
    ///    explicitly declares the display precision (e.g. `D 1.00 GOLD`
    ///    declares GOLD has 2 decimal places). This is the primary fix for
    ///    #288: wow.dat declares `D 1.00 GOLD` which establishes
    ///    `inferred_scale[GOLD] = 2` so that `@ 1.25 GOLD` costs are not
    ///    integer-rounded.
    ///
    /// 2. **Direct posting amounts** — the amounts written on the posting
    ///    line itself (not `@`/`@@` price annotations or `{cost}` lot costs).
    ///    This captures journals that use explicit fractional amounts
    ///    (e.g. `$474.31`) to establish commodity precision.
    ///
    /// `@`-price and `{cost}` lot annotations are intentionally excluded from
    /// the scale computation because they may carry IEEE-double noise from
    /// import tools (e.g. a 28-digit `@ $53.659999...` price would
    /// incorrectly inflate `inferred_scale[$]` to 28, defeating the rounding
    /// that eliminates floating-point residue from the balance check).
    pub inferred_scale: u32,
}

/// Properties of an account declared with an `account` directive.
///
/// Marked `#[non_exhaustive]`: external HIR-construction code can
/// build via `Default::default()` and mutate fields, but cannot use
/// struct-literal-with-spread syntax. The set of properties is
/// expected to grow as we wire more directive sub-items through
/// (e.g. commodity restrictions on `open`, account-level booking
/// overrides for #248-style features); the marker keeps additions
/// additive.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct AccountProperties {
    /// A free-form note describing the account.
    pub note: Option<String>,
    /// Fatal assertions: every posting to this account must satisfy all of
    /// these expressions. Elaboration halts if any fails.
    pub(crate) asserts: Vec<ast::BoolExpr>,
    /// Non-fatal checks: if any fail, a warning is printed to stderr but
    /// elaboration continues.
    pub(crate) checks: Vec<ast::BoolExpr>,
    /// Key-value metadata declared on this account directive only -- not
    /// yet inherited from ancestors. Sources include `; key: value`
    /// notes on the directive header and `key: value` sub-items inside
    /// the block. Elaboration denormalises by walking ancestors.
    pub metadata: BTreeMap<String, String>,
    /// Booking method declared on a Beancount `open` directive
    /// (e.g. `open Assets:Brokerage AAPL "FIFO"`). `None` when no
    /// method was specified — the elaborator falls back to the
    /// active [`ElaborationConfig`]'s default. See [`BookingMethod`].
    pub booking_method: Option<BookingMethod>,
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
    /// A Beancount `pad` marker, threaded through to the elaborator.
    ///
    /// The resolver does not act on pads; it only resolves the date so the
    /// elaborator can compare against the next balance assertion.
    Pad(PadDirective),
}

/// A Beancount `pad` directive with its date resolved.
///
/// The elaborator interprets a pad as: "if the next balance assertion on
/// `target_account` would otherwise fail, insert a balancing transaction
/// (back-dated to `date`) that posts the difference between
/// `target_account` and `source_account`."
#[derive(Debug)]
pub(crate) struct PadDirective {
    /// The date the pad applies on.
    pub date: chrono::NaiveDate,
    /// The account whose balance will be brought to the next assertion.
    pub target_account: String,
    /// The counter-account that absorbs the padding amount.
    pub source_account: String,
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
#[non_exhaustive]
pub enum ResolutionError {
    /// A date could not be resolved: either the year was absent and no
    /// fallback was available, or the resulting calendar date is invalid
    /// (e.g. February 30).
    InvalidDate,
    /// An automated transaction rule's query string could not be compiled
    /// as a regex. Carries the raw query and the underlying regex error.
    InvalidAutoRuleQuery(String, String),
    /// A cycle was detected among `C` commodity-conversion directives.
    ///
    /// The carried string names one commodity that participates in the cycle
    /// (e.g. `"X"` when `C 1X = 1Y` and `C 1Y = 1X` are both in scope).
    /// Chains like `c → s → G` are valid and are fully resolved to the chain
    /// root; a cycle has no root and cannot be resolved.
    CommodityConversionCycle(String),
    /// A `C` directive has a zero LHS amount, making the divisor formula
    /// `N2 / N1` undefined (division by zero).
    ///
    /// The two carried strings are the LHS and RHS commodity names from the
    /// offending directive, e.g. `("X", "Y")` for `C 0 X = 100 Y`.
    InvalidCommodityConversion(String, String),
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionError::InvalidDate => {
                write!(f, "Invalid date")
            }
            ResolutionError::InvalidAutoRuleQuery(query, err) => {
                write!(f, "Invalid auto-rule query `{query}`: {err}")
            }
            ResolutionError::CommodityConversionCycle(commodity) => {
                write!(
                    f,
                    "Cycle detected in C commodity-conversion directives \
                     involving commodity `{commodity}`"
                )
            }
            ResolutionError::InvalidCommodityConversion(lhs, rhs) => {
                write!(
                    f,
                    "C directive `C 0 {lhs} = ... {rhs}` has a zero LHS amount; \
                     divisor N2/N1 is undefined (division by zero)"
                )
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Compile an auto-rule query string into a regex.
///
/// `/pattern/` queries are compiled as-is (the delimiters are stripped);
/// any other form is treated as a case-insensitive substring match,
/// mirroring ledger-cli's behaviour for bare-string queries.
fn compile_auto_rule_query(query: &str) -> Result<regex::Regex, ResolutionError> {
    let pattern = if query.starts_with('/') && query.ends_with('/') && query.len() >= 2 {
        query[1..query.len() - 1].to_string()
    } else {
        format!("(?i){}", regex::escape(query))
    };
    regex::Regex::new(&pattern)
        .map_err(|e| ResolutionError::InvalidAutoRuleQuery(query.to_string(), e.to_string()))
}

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
                // ":tag1:tag2:" -- split on ":" to get individual tags
                for tag in note.split(":") {
                    tags.push(tag.into());
                }
            } else if let Some((key, value)) = note.split_once(":") {
                // "key: value" -- insert as metadata
                metadata.insert(key.trim().into(), value.trim().into());
            } else {
                // Plain comment -- preserve rather than discard
                comments.push(note.to_string());
            }
        }
        (tags, metadata, comments)
    }
}

/// Build an [`HIR`] from externally-constructed transactions and prices.
///
/// [`HIR`] is the canonical input to [`crate::frontend::Frontend::write_journal`].
/// Its internal shape (per-entry `context_id` indices, alias/commodity context
/// table) is a resolution-stage artifact that downstream writers don't consume
/// but that external callers can't populate sensibly. `HirBuilder` hides those
/// internals and exposes a small, typed surface for the use case of "I have
/// transactions in memory; serialise them as ledger / hledger / beancount source
/// text."
///
/// HIRs built with `HirBuilder` are intended for the write path only. Passing
/// a builder-produced HIR to [`crate::elaborate`] may panic because the
/// contexts table is empty and the elaborator dereferences it by index.
///
/// `push_assertion` and `push_pad` are not currently provided; open an issue if
/// your use case requires them.
///
/// # Example
///
/// ```rust
/// use doppio::resolution::{HirBuilder, Transaction, Posting};
/// use doppio::frontend::Frontend as _;
/// use doppio::LedgerFrontend;
/// use chrono::NaiveDate;
/// use rust_decimal::Decimal;
///
/// let txn = Transaction::new(
///     NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
///     "Groceries",
/// )
/// .with_posting(
///     Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "USD")),
/// )
/// .with_posting(Posting::new("Assets:Checking"));
///
/// let hir = HirBuilder::new().push_transaction(txn).build();
///
/// let mut out = Vec::new();
/// LedgerFrontend.write_journal(&hir, &mut out).unwrap();
/// let text = String::from_utf8(out).unwrap();
/// assert!(text.contains("Groceries"));
/// ```
#[derive(Debug, Default)]
pub struct HirBuilder {
    entries: Vec<ResolutionEntry>,
    prices: Vec<HistoricalPrice>,
}

impl HirBuilder {
    /// Creates a new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a transaction to the builder (builder pattern).
    pub fn push_transaction(mut self, txn: Transaction) -> Self {
        self.entries.push(ResolutionEntry {
            context_id: 0,
            data: Entry::Transaction(txn),
        });
        self
    }

    /// Appends a historical price directive to the builder (builder pattern).
    pub fn push_price(mut self, price: HistoricalPrice) -> Self {
        self.prices.push(price);
        self
    }

    /// Consumes the builder and returns a finished [`HIR`].
    ///
    /// The returned HIR has an empty contexts table (sufficient for the write
    /// path) and no auto-rules.
    pub fn build(self) -> HIR {
        HIR {
            entries: self.entries,
            prices: self.prices,
            // One empty context so context_id 0 is always a valid index,
            // matching the invariant maintained by HIR::default().
            contexts: vec![Context::default()],
            global_context: GlobalContext::default(),
            auto_rules: vec![],
        }
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
                ast::Entry::Pad(p) => {
                    let date = Self::resolve_date(&p.date, current_default_year)?;
                    let data = Entry::Pad(PadDirective {
                        date,
                        target_account: p.target_account,
                        source_account: p.source_account,
                    });
                    result.entries.push(ResolutionEntry { context_id, data });
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
                                        ctx.commodity_conversions.insert(
                                            alias,
                                            (name.clone(), rust_decimal::Decimal::ONE),
                                        );
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
                                // Extract the decimal scale from the format
                                // string and use it as a precision anchor for
                                // `inferred_scale`. This is the primary fix for
                                // #288: `D 1.00 GOLD` in wow.dat lowers to a
                                // `Format("1.00 GOLD")` item that declares
                                // GOLD has scale 2, even though the direct
                                // GOLD postings in that journal are integer
                                // amounts (scale 0). The elaborator uses
                                // `inferred_scale` when rounding `qty * @price`
                                // costs for GOLD, so without this the
                                // integer scale 0 would over-round fractional
                                // GOLD values computed from `@`-price annotations.
                                //
                                // `scale_from_format` counts digits after the
                                // first decimal point in the format string,
                                // ignoring grouping separators. A format with
                                // no decimal point contributes scale 0.
                                let fmt_scale = scale_from_format(&format);
                                global_context.inferred_scale =
                                    global_context.inferred_scale.max(fmt_scale);
                                global_context.format = Some(format);
                            }
                            ast::CommodityItem::NoMarket => {
                                global_context.no_market = true;
                            }
                            ast::CommodityItem::Note(note) => {
                                global_context.note = Some(note);
                            }
                            // The `Note` arm above now handles all note values;
                            // the `Unknown("note", ...)` path is superseded.
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
                    // form comments on accounts are dropped -- they have no
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
                            ast::AccountItem::Booking(method) => {
                                global_context.booking_method = Some(method);
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
                    let postings: Vec<Posting> = transaction
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

                    // Populate `inferred_scale` from direct posting amounts.
                    //
                    // This is a side-effect of the AST → HIR walk; the
                    // elaborator reads the result from
                    // `global_context.commodity_properties` to round
                    // `qty * price` costs for `@`-priced postings (#281).
                    //
                    // Only the `value` field of each `AmountDetails::Amount`
                    // posting contributes here — NOT `@`/`@@` price annotations
                    // and NOT `{cost}` lot annotations. This prevents
                    // IEEE-double noise from high-precision price strings
                    // (e.g. `@ $53.6599999999999999998612221219`, 28 digits)
                    // from inflating the inferred scale of `$` beyond the
                    // user's true intent. (#281 anchor preserved for
                    // standard.dat:1880)
                    //
                    // `D` / `commodity format` directives are the other
                    // authoritative source of scale — they are handled in
                    // the `CommodityItem::Format` arm above (#288 fix for
                    // wow.dat where `D 1.00 GOLD` establishes scale 2
                    // even though direct GOLD postings are integers).
                    {
                        let mut scales: BTreeMap<String, u32> = BTreeMap::new();
                        for posting in &postings {
                            if let Some(ast::AmountDetails::Amount { value, .. }) = &posting.amount
                            {
                                collect_scales_from_expr(value, &mut scales);
                            }
                        }
                        for (commodity, scale) in scales {
                            let props = result
                                .global_context
                                .commodity_properties
                                .entry(commodity)
                                .or_default();
                            props.inferred_scale = props.inferred_scale.max(scale);
                        }
                    }

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
                ast::Entry::AutoRule(rule) => {
                    // Collect into `auto_rules` rather than `entries` so the
                    // elaborator can apply them to every transaction without
                    // them appearing in the chronological entry stream.
                    let query = compile_auto_rule_query(&rule.query)?;
                    let postings = rule
                        .postings
                        .into_iter()
                        .map(|p| ResolvedAutoRulePosting {
                            account: p.account,
                            amount: p.amount,
                        })
                        .collect();
                    result.auto_rules.push(ResolvedAutoRule { query, postings });
                }
                ast::Entry::CommodityConversion { lhs, rhs } => {
                    // `C N1 X = N2 Y` — X (LHS commodity) is canonical, Y (RHS) is alias.
                    //
                    // Principled-math derivation: N1 units of X = N2 units of Y, so
                    //   1 Y = (N1/N2) X, i.e. a Y-amount divides by (N2/N1) to get X.
                    // Therefore: divisor = N2 / N1 (#274).
                    //
                    // Examples:
                    //   C 1 SLV = 100c  -> divisor = 100/1 = 100;  250c / 100 = 2.50 SLV
                    //   C 100 SLV = 1 G -> divisor = 1/100 = 0.01; 1G / 0.01 = 100 SLV
                    //
                    // N1 = 0 is rejected as division by zero.
                    //
                    // We insert TWO entries per directive:
                    //   (rhs.commodity, (lhs.commodity, N2/N1))
                    //     — converts RHS-commodity postings into LHS canonical
                    //   (lhs.commodity, (lhs.commodity, N1))
                    //     — scales postings already in the canonical commodity by N1
                    //     (C 100c = 1s, posting 250c -> 2.5c)
                    //
                    // After inserting, we run a fixpoint pass that transitively
                    // resolves all chains to their root commodity (fix for #266).
                    // For example, with `C 1s = 100c` and `C 1G = 100s` in scope,
                    // the `c` entry resolves all the way to G (divisor = 10000),
                    // so a posting in `c` will elaborate directly to G.
                    //
                    // Cycles (C 1X = 1Y; C 1Y = 1X) are detected and surfaced as
                    // ResolutionError::CommodityConversionCycle.
                    let divisor = rhs.value.checked_div(lhs.value).ok_or_else(|| {
                        ResolutionError::InvalidCommodityConversion(
                            lhs.commodity.clone(),
                            rhs.commodity.clone(),
                        )
                    })?;
                    let mut ctx = new_context.unwrap_or_else(|| context.clone());
                    // RHS commodity -> LHS canonical with divisor = N2/N1.
                    //
                    // Use `insert` (overwrite) so that a later directive like
                    // `C 1G = 100s` correctly updates s's entry to point at G,
                    // even if s was previously inserted as a self-loop sentinel
                    // by an earlier directive (`C 1s = 100c` inserts s→(s,1)).
                    ctx.commodity_conversions
                        .insert(rhs.commodity.clone(), (lhs.commodity.clone(), divisor));
                    // LHS commodity -> itself with divisor = 1 (self-loop sentinel).
                    //
                    // The self-loop marks the LHS as the chain root so the fixpoint
                    // walk can terminate. Divisor = 1 (identity) so that postings
                    // already in the canonical commodity are not rescaled — principled
                    // math: `C N1 X = N2 Y` means 1X = (N2/N1)Y; a posting of A X
                    // remains A X with no divisor applied.
                    //
                    // Use `entry(...).or_insert_with` so a more specific
                    // canonical-resolution inserted by an earlier directive is
                    // not overwritten: if `s` is already resolved (e.g. because
                    // `C 1s = 100c` ran first and s→(s,1) is set, then
                    // `C 1G = 100s` runs and updates s→(G,100) via the rhs insert
                    // above), the lhs self-loop insert here is a no-op for
                    // entries that already have a non-self-loop target.
                    ctx.commodity_conversions
                        .entry(lhs.commodity.clone())
                        .or_insert_with(|| (lhs.commodity.clone(), rust_decimal::Decimal::ONE));
                    // Run the transitive fixpoint: chain c -> s -> G becomes
                    // c -> G directly (with product divisor).  Cycles error out.
                    ctx.resolve_commodity_conversion_chains()?;
                    new_context = Some(ctx);
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

/// Walk a [`ast::ValueExpr`] and update `scales` with the maximum decimal
/// scale seen for each commodity-bearing amount leaf.
///
/// Called during the AST → HIR resolution walk (in [`HIR::try_from`]) for
/// direct posting amounts. The result is stored on
/// [`CommodityProperties::inferred_scale`] and consumed by the elaborator
/// when rounding `qty * price` costs for `@`-priced postings to the
/// journal's intended precision (see `crates/doppio/src/elaborator.rs`).
///
/// Only the `value` field of each posting is passed here — NOT `@`/`@@` price
/// annotations or `{cost}` lot costs. This prevents IEEE-double noise from
/// high-precision price strings from inflating the inferred scale.
/// `D` / `commodity format` directives also feed `inferred_scale` directly
/// (see [`scale_from_format`]).
///
/// The algorithm records the **maximum** scale across all direct posting amounts
/// for each commodity. This preserves the highest declared precision: a journal
/// with `2 B` (scale 0) and `-0.71 B` (scale 2) produces `inferred_scale = 2`
/// for `B`, so costs in `B` are rounded to 2 decimal places. Using `min` would
/// coarsen `0.71 B` to `1 B` (incorrect).
fn collect_scales_from_expr(expr: &ast::ValueExpr, scales: &mut BTreeMap<String, u32>) {
    match expr {
        ast::ValueExpr::Amount {
            value,
            commodity: Some(c),
        } => {
            let entry = scales.entry(c.clone()).or_insert(0);
            *entry = (*entry).max(value.scale());
        }
        ast::ValueExpr::Amount {
            commodity: None, ..
        } => {}
        ast::ValueExpr::Unary { expr, .. } => collect_scales_from_expr(expr, scales),
        ast::ValueExpr::Binary { lhs, rhs, .. } => {
            collect_scales_from_expr(lhs, scales);
            collect_scales_from_expr(rhs, scales);
        }
        // `Typed { expr, commodity }` carries the commodity as an outer annotation.
        // The inner `expr` is a bare-number tree (no commodity on any leaf).
        // Extract the leaf scale and record it under the outer commodity.
        ast::ValueExpr::Typed { expr, commodity } => {
            if let Some(scale) = bare_number_scale(expr) {
                let entry = scales.entry(commodity.clone()).or_insert(0);
                *entry = (*entry).max(scale);
            }
            // Also recurse in case the sub-expression itself contains commodity
            // leaves (unusual but correct to handle).
            collect_scales_from_expr(expr, scales);
        }
        ast::ValueExpr::Group(bool_expr) => {
            collect_scales_from_expr(&bool_expr.lhs, scales);
            if let Some((_, rhs)) = &bool_expr.cmp {
                collect_scales_from_expr(rhs, scales);
            }
        }
        // Object, Commodity, Str, Regex, Function, Access — no commodity-bearing
        // amount leaf to record.
        _ => {}
    }
}

/// Extract the decimal scale from a bare-number (no-commodity) value expression.
///
/// Returns `Some(scale)` when the expression reduces to a single numeric leaf
/// (`Amount { commodity: None, .. }`) optionally wrapped in a unary `+`/`-`.
/// Returns `None` for complex expressions (arithmetic, function calls, etc.)
/// where the scale cannot be determined without evaluation.
fn bare_number_scale(expr: &ast::ValueExpr) -> Option<u32> {
    match expr {
        ast::ValueExpr::Amount {
            value,
            commodity: None,
        } => Some(value.scale()),
        ast::ValueExpr::Unary { expr, .. } => bare_number_scale(expr),
        _ => None,
    }
}

/// Extract the decimal scale declared in a commodity format string.
///
/// A format string is the raw source text of a `D` directive or a `format`
/// sub-item inside a `commodity` block — for example `"1.00 GOLD"`,
/// `"$1,000.00"`, or `"1,000.00 USD"`.
///
/// The scale is the count of ASCII digit characters immediately following the
/// first `.` in the string, ignoring all other characters (currency symbols,
/// grouping commas, commodity names). If the string contains no `.`, the
/// format declares zero decimal places and `0` is returned.
///
/// # Examples
///
/// ```text
/// "1.00 GOLD"   -> 2
/// "$1,000.00"   -> 2
/// "1,000.00 $"  -> 2
/// "1.0000 EUR"  -> 4
/// "1000 USD"    -> 0
/// ```
fn scale_from_format(format: &str) -> u32 {
    if let Some(dot_pos) = format.find('.') {
        let after_dot = &format[dot_pos + 1..];
        after_dot.chars().take_while(|c| c.is_ascii_digit()).count() as u32
    } else {
        0
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
            hir.contexts[1]
                .commodity_conversions
                .get("Bitcoin")
                .unwrap()
                .0,
            "BTC"
        );
        // Verify context 0 does not
        assert!(hir.contexts[0].commodity_conversions.is_empty());
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

    // ──────────────────────────────────────────────────────────────────────────
    // inferred_scale populated in HIR (#281)
    // ──────────────────────────────────────────────────────────────────────────

    /// Verify that `inferred_scale` is populated on `CommodityProperties`
    /// during the AST → HIR resolution walk — not by the elaborator.
    ///
    /// A journal with `$1.00`, `$2.50`, and `$3.00` direct posting amounts
    /// must produce `inferred_scale == 2` for `$` (max of scales 2, 2, 2).
    #[test]
    fn test_inferred_scale_populated_in_hir() {
        use rust_decimal::dec;

        let make_posting = |value: rust_decimal::Decimal, commodity: &str| -> ast::Posting {
            ast::Posting::new("Assets:Test").with_amount(ast::AmountDetails::Amount {
                value: ast::ValueExpr::Amount {
                    value,
                    commodity: Some(commodity.into()),
                },
                lot_annotation: None,
                lot_pricing: None,
                balance_assertion: None,
            })
        };

        let tx = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Scale test".into(),
            notes: vec![],
            postings: vec![
                make_posting(dec!(1.00), "$"),
                make_posting(dec!(2.50), "$"),
                make_posting(dec!(3.00), "$"),
                // Counter-posting to balance (no commodity annotation so
                // no scale contribution).
                ast::Posting::new("Assets:Other"),
            ],
            ..Default::default()
        };

        let journal = ast::Journal {
            entries: vec![ast::Entry::Transaction(tx)],
        };

        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("$")
            .expect("$ should have properties after resolution");

        assert_eq!(
            props.inferred_scale, 2,
            "inferred_scale for $ must be 2 (max of 2, 2, 2 from $1.00, $2.50, $3.00)"
        );
    }

    /// Regression: hledger suffix-commodity amounts like `-0.71 B` parse as
    /// `Typed { Unary { Sub, Amount { 0.71, None } }, "B" }`. The scale
    /// collector must handle this form; otherwise `B` is absent from the
    /// commodity-properties map and canonical-cost rounding is skipped.
    #[test]
    fn test_inferred_scale_hledger_typed_node() {
        use rust_decimal::dec;

        // Simulate what the hledger parser produces for `-0.71 B`
        let typed_amount = ast::AmountDetails::Amount {
            value: ast::ValueExpr::Typed {
                expr: Box::new(ast::ValueExpr::Unary {
                    op: ast::Op::Sub,
                    expr: Box::new(ast::ValueExpr::Amount {
                        value: dec!(0.71),
                        commodity: None,
                    }),
                }),
                commodity: "B".into(),
            },
            lot_annotation: None,
            lot_pricing: None,
            balance_assertion: None,
        };

        let tx = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Typed node test".into(),
            notes: vec![],
            postings: vec![
                ast::Posting::new("Assets:A").with_amount(typed_amount),
                ast::Posting::new("Assets:B"),
            ],
            ..Default::default()
        };

        let journal = ast::Journal {
            entries: vec![ast::Entry::Transaction(tx)],
        };

        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("B")
            .expect("B should have properties after resolution of Typed node");

        assert_eq!(
            props.inferred_scale, 2,
            "inferred_scale for B must be 2 from the Typed{{Unary{{Sub, 0.71}}}} node"
        );
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Format-directive scale feeds inferred_scale (#288)
    // ──────────────────────────────────────────────────────────────────────────

    /// A `D 1.00 GOLD` (or `commodity GOLD\n  format 1.00 GOLD`) directive
    /// declares that GOLD has 2 decimal places. This scale must be recorded
    /// in `inferred_scale` even when the only direct GOLD postings in the
    /// journal are integers (scale 0).
    ///
    /// This is the root cause of #288: wow.dat has `D 1.00 GOLD` but all
    /// direct GOLD postings are integers (`1 GOLD`, `3 GOLD`, …). Without
    /// the format-directive contribution, `inferred_scale[GOLD] = 0`, causing
    /// `qty * @price` GOLD costs to be integer-rounded instead of
    /// 2-decimal-rounded.
    #[test]
    fn test_inferred_scale_from_format_directive() {
        // `D 1.00 GOLD` lowers to a Commodity directive with Format("1.00 GOLD")
        // and Default items.
        let journal = ast::Journal {
            entries: vec![ast::Entry::Directive(ast::Directive::Commodity {
                name: "GOLD".into(),
                notes: vec![],
                items: vec![
                    ast::CommodityItem::Default,
                    ast::CommodityItem::Format("1.00 GOLD".into()),
                ],
            })],
        };

        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("GOLD")
            .expect("GOLD should have properties from D directive");

        assert_eq!(
            props.inferred_scale, 2,
            "inferred_scale for GOLD must be 2 from D 1.00 GOLD format directive"
        );
    }

    /// When BOTH a format directive AND direct posting amounts exist for a
    /// commodity, `inferred_scale` takes the maximum of both. This preserves
    /// the highest declared precision.
    ///
    /// Example: `D 1.000 GOLD` (scale 3) + direct `1.00 GOLD` (scale 2) →
    /// `inferred_scale[GOLD] = 3`.
    #[test]
    fn test_inferred_scale_format_and_direct_max() {
        use rust_decimal::dec;

        let format_directive = ast::Entry::Directive(ast::Directive::Commodity {
            name: "GOLD".into(),
            notes: vec![],
            items: vec![ast::CommodityItem::Format("1.000 GOLD".into())],
        });

        let tx = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Format + direct max test".into(),
            postings: vec![
                ast::Posting::new("Assets:Wallet").with_amount(ast::AmountDetails::Amount {
                    value: ast::ValueExpr::Amount {
                        value: dec!(1.00),
                        commodity: Some("GOLD".into()),
                    },
                    lot_annotation: None,
                    lot_pricing: None,
                    balance_assertion: None,
                }),
                ast::Posting::new("Assets:Cash"),
            ],
            ..Default::default()
        };

        let journal = ast::Journal {
            entries: vec![format_directive, ast::Entry::Transaction(tx)],
        };
        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("GOLD")
            .expect("GOLD should have properties");

        assert_eq!(
            props.inferred_scale, 3,
            "inferred_scale for GOLD must be 3 (max of format scale 3 and direct scale 2)"
        );
    }

    /// `@`-price annotations are NOT included in `inferred_scale`. Only
    /// direct posting amounts and format directives contribute. This prevents
    /// IEEE-double noise from high-precision price strings from inflating the
    /// apparent scale of a commodity that already has clear precision from
    /// direct postings.
    ///
    /// Example: `1 GOLD` direct (scale 0) + `@ 1.250 GOLD` @-price (scale 3)
    /// → `inferred_scale[GOLD] = 0` (direct only; @-price is not consulted).
    #[test]
    fn test_inferred_scale_at_price_not_included() {
        use rust_decimal::dec;

        // Direct posting: 1 GOLD (scale 0)
        let posting_direct =
            ast::Posting::new("Assets:Wallet").with_amount(ast::AmountDetails::Amount {
                value: ast::ValueExpr::Amount {
                    value: dec!(1),
                    commodity: Some("GOLD".into()),
                },
                lot_annotation: None,
                lot_pricing: None,
                balance_assertion: None,
            });

        // Posting with @-price annotation: 1 ITEM @ 1.250 GOLD (scale 3)
        // The @-price must NOT affect inferred_scale[GOLD].
        let posting_at =
            ast::Posting::new("Assets:Items").with_amount(ast::AmountDetails::Amount {
                value: ast::ValueExpr::Amount {
                    value: dec!(1),
                    commodity: Some("ITEM".into()),
                },
                lot_annotation: None,
                lot_pricing: Some(ast::LotPricing::Unit(ast::ValueExpr::Amount {
                    value: dec!(1.250),
                    commodity: Some("GOLD".into()),
                })),
                balance_assertion: None,
            });
        let counter = ast::Posting::new("Assets:Cash");

        let tx = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "At-price not included test".into(),
            postings: vec![posting_direct, posting_at, counter],
            ..Default::default()
        };

        let journal = ast::Journal {
            entries: vec![ast::Entry::Transaction(tx)],
        };
        let hir = HIR::try_from(journal).unwrap();

        let props = hir
            .global_context
            .commodity_properties
            .get("GOLD")
            .expect("GOLD should have properties from direct posting");

        assert_eq!(
            props.inferred_scale, 0,
            "inferred_scale for GOLD must be 0 (direct posting only; @-price scale 3 excluded)"
        );
    }

    /// `{cost}` lot annotations are NOT included in `inferred_scale`.
    ///
    /// Example: `1 AAPL {150.50 USD}` with no direct USD postings and no
    /// format directive → `inferred_scale[USD] = 0` (lot cost is not consulted).
    #[test]
    fn test_inferred_scale_lot_cost_not_included() {
        use rust_decimal::dec;

        let posting =
            ast::Posting::new("Assets:Brokerage").with_amount(ast::AmountDetails::Amount {
                value: ast::ValueExpr::Amount {
                    value: dec!(1),
                    commodity: Some("AAPL".into()),
                },
                lot_annotation: Some(ast::LotAnnotation {
                    cost: Some(ast::ValueExpr::Amount {
                        value: dec!(150.50),
                        commodity: Some("USD".into()),
                    }),
                    ..Default::default()
                }),
                lot_pricing: None,
                balance_assertion: None,
            });
        let counter = ast::Posting::new("Assets:Cash");

        let tx = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Lot cost not included test".into(),
            postings: vec![posting, counter],
            ..Default::default()
        };

        let journal = ast::Journal {
            entries: vec![ast::Entry::Transaction(tx)],
        };
        let hir = HIR::try_from(journal).unwrap();

        // USD should NOT appear in commodity_properties at all (no direct
        // posting or format directive introduced USD).
        let has_usd = hir
            .global_context
            .commodity_properties
            .get("USD")
            .map(|p| p.inferred_scale > 0)
            .unwrap_or(false);

        assert!(
            !has_usd,
            "inferred_scale for USD must NOT be populated from {{cost}} lot annotations"
        );
    }
}
