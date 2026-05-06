//! Elaboration stage: evaluate expressions, balance transactions, and
//! produce the final serialisable [`Journal`].
//!
//! This stage converts a [`resolution::HIR`] into an [`elaborator::Journal`]
//! by performing the following work:
//!
//! - **Expression evaluation** -- [`ast::ValueExpr`] trees are evaluated to
//!   concrete `(Decimal, commodity)` pairs by the [`evaluator`] submodule.
//!   Commodity aliases from the active [`resolution::Context`] are applied.
//!
//! - **Transaction balancing** -- if a transaction has exactly one posting with
//!   no explicit amount (a "null posting"), its amount is inferred as the
//!   negation of all other postings' sum. If all postings have amounts their
//!   sum must be zero; otherwise [`ElaborationError::TransactionDoesNotBalance`]
//!   is returned.
//!
//! - **Balance assertions / assignments** -- `= expected` checks are verified
//!   against the running account balance. `= target` assignments set the
//!   posting amount to `target − current_balance`.
//!
//! - **Lot pricing** -- `@ unit` and `@@ total` cost annotations are converted
//!   into a cash amount in the lot's commodity for the purpose of balancing.
//!
//! - **Account registration** -- every account mentioned in a posting is added
//!   to [`Journal::accounts`], merging any properties declared in `account`
//!   directives.

use std::{collections::BTreeMap, fmt::Display};

use rust_decimal::Decimal;

use crate::{
    ast::{self, AmountDetails, ValueExpr},
    resolution,
};

/// Per-account running balance, used during elaboration to evaluate balance
/// assertions and the `account()` expression function.
#[derive(Default, Clone, Debug)]
struct AccountBalances {
    /// The balance for each commodity held in this account.
    commodity: BTreeMap<String, Decimal>,
}

/// Mutable state threaded through the elaboration of all transactions.
///
/// Account balances are updated as each transaction is processed so that
/// balance assertions and the `account()` function see the balance *before*
/// the current posting is applied (which matches ledger-cli semantics).
#[derive(Default, Clone, Debug)]
struct RunningState {
    account_balances: BTreeMap<String, AccountBalances>,
}

/// A commodity name (e.g. `"USD"`, `"BTC"`, `"$"`).
pub type Commodity = String;

/// A multi-commodity amount: a map from commodity symbol to a `Decimal` value.
#[derive(Default, Debug)]
pub struct Amount(pub BTreeMap<Commodity, Decimal>);

/// Cleared/pending state of a resolved transaction or posting.
#[derive(Debug)]
pub enum TransactionState {
    /// No state marker.
    Uncleared,
    /// `!` -- pending confirmation.
    Pending,
    /// `*` -- confirmed / reconciled.
    Cleared,
}

impl From<ast::TransactionState> for TransactionState {
    fn from(f: ast::TransactionState) -> TransactionState {
        match f {
            ast::TransactionState::Uncleared => TransactionState::Uncleared,
            ast::TransactionState::Pending => TransactionState::Pending,
            ast::TransactionState::Cleared => TransactionState::Cleared,
        }
    }
}

/// Errors that can occur during the elaboration stage.
#[derive(Debug)]
pub enum ElaborationError {
    /// A posting amount evaluated to a bare number with no commodity, and no
    /// default commodity was set in the active context.
    AmountWithNoCommodity,
    /// A value expression evaluated to a non-amount type (e.g. a string or
    /// object) where an amount was expected.
    NonAmountWhereAmountExpected(ValueExpr),
    /// An error from the expression evaluator.
    EvaluationError(EvaluationError),
    /// A `= expected` balance assertion on a posting failed: the account's
    /// balance after this posting does not equal the asserted value.
    PostingBalanceAssertionFailed,
    /// A standalone balance assertion directive failed: the account's balance
    /// at the assertion's position in the file does not match the expected
    /// amount.
    BalanceAssertionFailed {
        /// The account whose balance was asserted.
        account: String,
        /// The date of the assertion directive.
        date: chrono::NaiveDate,
        /// The amount the assertion expected.
        expected_amount: Decimal,
        /// The commodity of the expected amount.
        expected_commodity: String,
        /// The actual balance of the account in that commodity.
        actual_amount: Decimal,
    },
    /// A transaction has more than one null posting (only one is allowed, since
    /// multiple unknowns cannot be uniquely determined).
    TooManyNullPostings,
    /// All postings have explicit amounts but they do not sum to zero.
    TransactionDoesNotBalance(Amount),
    /// An `assert` expression on an account directive evaluated to `false`
    /// for a posting to that account.
    AccountAssertionFailed {
        /// The account whose assertion fired.
        account: String,
        /// Zero-based index of the posting within the transaction.
        posting_index: usize,
        /// A rendered form of the failing expression (for diagnostics).
        ///
        /// Produced via the AST's `Display` impl, so formatting may be
        /// normalized (e.g. whitespace, `$500` rendered as `500 $`) and may
        /// not byte-match the original source text.
        expression: String,
    },
    /// A `tag` directive `assert` expression evaluated to `false` for a
    /// `; TagName: value` metadata pair on a transaction or posting.
    TagAssertionFailed {
        /// The tag name whose assertion fired (e.g. `"Statement"`).
        tag_name: String,
        /// The metadata value that failed the assertion (e.g. `"foo/bar"`).
        tag_value: String,
        /// A rendered form of the failing expression (for diagnostics).
        expression: String,
    },
}

/// Error produced when evaluating a value expression (e.g., an amount or
/// balance assertion expression) fails.
///
/// This error is always wrapped in [`ElaborationError::EvaluationError`]; it
/// is unlikely to be matched directly by callers.
#[derive(Debug)]
pub enum EvaluationError {
    /// `*` or `/` used as a unary prefix operator, which is not meaningful.
    UnaryMultiplyOrDivide,
    /// A unary operator was applied to a non-amount value (e.g. a string).
    UnaryOnNonAmount(ValueExpr),
    /// A binary operator was applied to incompatible types or mismatched
    /// commodities (e.g. `USD + EUR`).
    BinaryOperationTypeError((ValueExpr, ValueExpr, crate::ast::Op)),
    /// Field access on an object referenced a field that does not exist.
    NoSuchField(String),
    /// Field access was attempted on a non-object value.
    FieldAccessTypeError(ValueExpr),
    /// A function call with unrecognised name or argument count.
    UnknownFunctionArgs((String, Vec<ValueExpr>)),
    /// A `Typed` annotation specified a commodity that is incompatible with
    /// the inner expression's commodity.
    TypedCommodityToIncompatibleAmount((String, ValueExpr)),
    /// A function received an argument of the wrong type.
    InvalidFunctionArgs((String, ValueExpr)),
    /// A regex literal could not be compiled.
    ///
    /// Carries the offending pattern string and the error message from the
    /// `regex` crate. Using `String` rather than `regex::Error` keeps this
    /// error type free of a public dependency on the `regex` crate.
    InvalidRegexPattern(String, String),
    /// A define with a boolean body was called from a value expression context.
    ///
    /// Boolean defines (e.g. `define pos(x) = x > 0`) may only be used where a
    /// `bool_expr` is expected (e.g. inside an `assert` directive), not as part
    /// of an arithmetic expression.
    BoolDefineInValueContext(String),
    /// A parameterized define was called with the wrong number of arguments.
    DefineArgCountMismatch {
        /// The define name.
        name: String,
        /// Number of parameters the define declares.
        expected: usize,
        /// Number of arguments provided at the call site.
        got: usize,
    },
    /// Expression evaluation exceeded the recursion limit. The most common
    /// cause is mutually-recursive defines, e.g. `define a = b; define b = a`.
    RecursionLimitExceeded,
}

impl Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvaluationError::UnaryMultiplyOrDivide => {
                write!(f, "* and / cannot be used as unary prefix operators")
            }
            EvaluationError::UnaryOnNonAmount(val) => {
                write!(f, "unary operator applied to non-amount value: {val:?}")
            }
            EvaluationError::BinaryOperationTypeError((lhs, rhs, op)) => {
                write!(f, "binary operation type mismatch: {lhs:?} {op:?} {rhs:?}")
            }
            EvaluationError::NoSuchField(field) => {
                write!(f, "no such field: {field}")
            }
            EvaluationError::FieldAccessTypeError(val) => {
                write!(f, "field access on non-object value: {val:?}")
            }
            EvaluationError::UnknownFunctionArgs((name, args)) => {
                write!(
                    f,
                    "unknown function or wrong argument count: {name}({args:?})"
                )
            }
            EvaluationError::TypedCommodityToIncompatibleAmount((commodity, val)) => {
                write!(
                    f,
                    "commodity annotation '{commodity}' is incompatible with value: {val:?}"
                )
            }
            EvaluationError::InvalidFunctionArgs((name, arg)) => {
                write!(f, "invalid argument to function {name}: {arg:?}")
            }
            EvaluationError::InvalidRegexPattern(pattern, err) => {
                write!(f, "invalid regex pattern /{pattern}/: {err}")
            }
            EvaluationError::BoolDefineInValueContext(name) => {
                write!(
                    f,
                    "define '{name}' has a boolean body and cannot be used in a value expression"
                )
            }
            EvaluationError::DefineArgCountMismatch {
                name,
                expected,
                got,
            } => {
                write!(
                    f,
                    "define '{name}' expects {expected} argument(s), got {got}"
                )
            }
            EvaluationError::RecursionLimitExceeded => {
                write!(
                    f,
                    "expression evaluation exceeded recursion limit (likely a cyclic `define`)"
                )
            }
        }
    }
}

impl From<EvaluationError> for ElaborationError {
    fn from(e: EvaluationError) -> ElaborationError {
        ElaborationError::EvaluationError(e)
    }
}

impl std::error::Error for ElaborationError {}

impl Display for ElaborationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElaborationError::AmountWithNoCommodity => {
                write!(f, "amount has no commodity and no default commodity is set")
            }
            ElaborationError::NonAmountWhereAmountExpected(expr) => {
                write!(f, "expected an amount but got: {expr:?}")
            }
            ElaborationError::EvaluationError(e) => {
                write!(f, "evaluation error: {e}")
            }
            ElaborationError::PostingBalanceAssertionFailed => {
                write!(f, "posting balance assertion failed")
            }
            ElaborationError::BalanceAssertionFailed {
                account,
                date,
                expected_amount,
                expected_commodity,
                actual_amount,
            } => {
                write!(
                    f,
                    "balance assertion failed for account {account} on {date}: \
                     expected {expected_amount} {expected_commodity}, \
                     actual {actual_amount} {expected_commodity}"
                )
            }
            ElaborationError::AccountAssertionFailed {
                account,
                posting_index,
                expression,
            } => {
                write!(
                    f,
                    "account assertion failed for posting {posting_index} to {account}: \
                     assert {expression}"
                )
            }
            ElaborationError::TooManyNullPostings => {
                write!(f, "transaction has more than one null posting")
            }
            ElaborationError::TransactionDoesNotBalance(_) => {
                write!(f, "transaction does not balance")
            }
            ElaborationError::TagAssertionFailed {
                tag_name,
                tag_value,
                expression,
            } => {
                write!(
                    f,
                    "tag assertion failed for {tag_name}: \"{tag_value}\": assert {expression}"
                )
            }
        }
    }
}

impl TryFrom<resolution::HIR> for crate::elaboration::Journal {
    type Error = ElaborationError;

    fn try_from(value: resolution::HIR) -> Result<Self, Self::Error> {
        let mut state = RunningState::default();

        let mut transactions = vec![];

        // Pre-populate the accounts map from directives so that accounts
        // declared with notes but never posted to still appear in the output.
        // Metadata is denormalised after the entry loop -- see end of fn.
        let mut accounts = BTreeMap::new();
        for (name, properties) in &value.global_context.account_properties {
            accounts.insert(
                name.clone(),
                crate::elaboration::AccountProperties {
                    note: properties.note.clone(),
                    metadata: properties
                        .metadata
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                },
            );
        }

        for entry in value.entries {
            let entry_context = &value.contexts[entry.context_id];
            match entry.data {
                resolution::Entry::Assertion(assertion) => {
                    // Evaluate the expected amount expression.
                    let (expected_amount, expected_commodity) =
                        evaluator::eval_and_normalize_amount(
                            assertion.amount,
                            entry_context,
                            &state,
                        )?;

                    // Look up the account's current balance for this commodity.
                    let actual_amount = state
                        .account_balances
                        .get(&assertion.account)
                        .and_then(|ab| ab.commodity.get(&expected_commodity))
                        .copied()
                        .unwrap_or(Decimal::ZERO);

                    // NOTE: strict (`==`) is currently treated identically to
                    // weak (`=`). Both check that the account balance for the
                    // specified commodity matches exactly. A future enhancement
                    // could make strict assertions also verify that the account
                    // holds no *other* commodities.
                    let _ = assertion.strict;

                    if actual_amount != expected_amount {
                        return Err(ElaborationError::BalanceAssertionFailed {
                            account: assertion.account,
                            date: assertion.date,
                            expected_amount,
                            expected_commodity,
                            actual_amount,
                        });
                    }
                }
                resolution::Entry::Transaction(mut transaction) => {
                    // `transaction_state` accumulates the running sum of all
                    // explicit posting amounts (per commodity) for balancing.
                    let mut transaction_state = Amount(BTreeMap::default());

                    // Prefer an explicit "payee:" metadata key; fall back to
                    // the transaction description as the default payee.
                    let payee = transaction
                        .metadata
                        .remove("payee")
                        .unwrap_or_else(|| transaction.description.clone());

                    // Two-pass approach: first evaluate all postings that have
                    // explicit amounts, accumulating the running sum. Null
                    // postings are collected for the second pass, where the
                    // single allowed null posting is filled in as the negation
                    // of the total.
                    let mut null_postings = vec![];
                    let mut resolved_postings = vec![];

                    for mut posting in transaction.postings {
                        let posting_kind = posting.kind;
                        if let Some(amount) = posting.amount {
                            let account_name = entry_context
                                .account_aliases
                                .get(&posting.account)
                                .cloned()
                                .unwrap_or(posting.account);
                            let account_balance = state.account_balances.get(&account_name);
                            // `lot_cash` -- the (total, commodity) pair to use for
                            // transaction balancing, along with the elaborated
                            // lot annotation for the proto output.
                            let (value, commodity, lot_cash, proto_lot) = match amount {
                                AmountDetails::Amount {
                                    value,
                                    lot_annotation,
                                    lot_pricing,
                                    balance_assertion,
                                } => {
                                    let (value, commodity) = evaluator::eval_and_normalize_amount(
                                        value,
                                        entry_context,
                                        &state,
                                    )?;

                                    // Evaluate the optional lot annotation (cost/date/note).
                                    // Preserved on the proto Posting regardless of which
                                    // path drives the cash balance.
                                    //
                                    // `cost_for_balance`: the evaluated (per_unit_cost,
                                    // cost_commodity) pair, kept separate so the
                                    // cash-balance fallback path can use it without
                                    // re-parsing the proto Amount.
                                    let (proto_lot, cost_for_balance) =
                                        if let Some(ann) = lot_annotation {
                                            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                                                .expect("epoch is valid");
                                            let proto_date =
                                                ann.date.map(|d| (d - epoch).num_days() as i32);
                                            let (proto_cost, cost_pair) =
                                                if let Some(cost_expr) = ann.cost {
                                                    let (cv, cc) =
                                                        evaluator::eval_and_normalize_amount(
                                                            cost_expr,
                                                            entry_context,
                                                            &state,
                                                        )?;
                                                    let proto_amount = crate::elaboration::Amount {
                                                        by_commodity: BTreeMap::from([(
                                                            cc.clone(),
                                                            crate::decimal_to_proto(cv),
                                                        )]),
                                                    };
                                                    (Some(proto_amount), Some((cv, cc)))
                                                } else {
                                                    (None, None)
                                                };
                                            let lot = crate::elaboration::Lot {
                                                cost: proto_cost,
                                                date: proto_date,
                                                note: ann.note,
                                            };
                                            (Some(lot), cost_pair)
                                        } else {
                                            (None, None)
                                        };

                                    // Cash-contribution priority:
                                    // 1. lot_pricing (@/@@) present  -> price drives cash (unchanged)
                                    // 2. lot_annotation.cost present -> quantity * cost_per_unit
                                    // 3. otherwise                   -> value contributes in its own
                                    //                                  commodity (today's fallback)
                                    let lot_cash = match lot_pricing {
                                        Some(ast::LotPricing::Total(expr)) => {
                                            let (mut v, c) = evaluator::eval_and_normalize_amount(
                                                expr,
                                                entry_context,
                                                &state,
                                            )?;
                                            // For a negative lot (selling), negate the cash total
                                            // so that it offsets correctly in transaction_state.
                                            if value.is_sign_negative() {
                                                v = -v;
                                            }
                                            Some((v, c))
                                        }
                                        Some(ast::LotPricing::Unit(expr)) => {
                                            // "@ unit_price" -- total cash = units * price
                                            let (v, c) = evaluator::eval_and_normalize_amount(
                                                expr,
                                                entry_context,
                                                &state,
                                            )?;
                                            Some((v * value, c))
                                        }
                                        None => {
                                            // No @/@@. If cost annotation is present, it drives
                                            // the cash balance: total = quantity * cost_per_unit.
                                            cost_for_balance.map(|(cv, cc)| (value * cv, cc))
                                        }
                                    };

                                    if let Some(balance_assertion) = balance_assertion {
                                        let (baval, bacommodity) =
                                            evaluator::eval_and_normalize_amount(
                                                balance_assertion,
                                                entry_context,
                                                &state,
                                            )?;
                                        // Assertion: current_balance + this_posting == expected.
                                        // The assertion is checked BEFORE the posting updates the
                                        // running state, so `account_balance` reflects the balance
                                        // *before* this posting -- consistent with ledger-cli.
                                        if !(bacommodity == commodity
                                            && account_balance
                                                .and_then(|ab| ab.commodity.get(&commodity))
                                                .unwrap_or(&Decimal::ZERO)
                                                + value
                                                == baval)
                                        {
                                            Err(ElaborationError::PostingBalanceAssertionFailed)?;
                                        }
                                    }
                                    (value, commodity, lot_cash, proto_lot)
                                }
                                AmountDetails::BalanceAssignment(assignment) => {
                                    // "= target_balance" -- compute the delta needed to reach the
                                    // target from the current running balance.
                                    //
                                    // If the assignment expression is bare (no commodity), try to
                                    // infer the commodity from, in order of preference:
                                    //   1. The account's existing running balance (single
                                    //      non-zero commodity) -- most common case.
                                    //   2. Other postings already processed in the current
                                    //      transaction (single commodity) -- covers e.g. bank
                                    //      imports that write `Income:Salary  =0` after a
                                    //      $-bearing `Assets:Checking` posting.
                                    // Running balances are never pruned, so we filter zero
                                    // entries to avoid stale `$=0` entries making a
                                    // single-commodity account look multi-commodity.
                                    let from_account = account_balance.and_then(|ab| {
                                        let mut non_zero = ab
                                            .commodity
                                            .iter()
                                            .filter(|(_, v)| !v.is_zero())
                                            .map(|(k, _)| k.as_str());
                                        let first = non_zero.next()?;
                                        non_zero.next().is_none().then_some(first)
                                    });
                                    let from_transaction = || {
                                        let mut keys = transaction_state.0.keys();
                                        let first = keys.next()?;
                                        keys.next().is_none().then_some(first.as_str())
                                    };
                                    let inferred_commodity = from_account.or_else(from_transaction);
                                    let (newsum, commodity) =
                                        evaluator::eval_and_normalize_amount_with_fallback(
                                            assignment,
                                            entry_context,
                                            &state,
                                            inferred_commodity,
                                        )?;
                                    let value = newsum
                                        - account_balance
                                            .and_then(|ab| ab.commodity.get(&commodity))
                                            .unwrap_or(&Decimal::ZERO);
                                    (value, commodity, None, None)
                                }
                            };
                            let payee = posting.metadata.remove("payee").unwrap_or(payee.clone());

                            // Virtual-unbalanced postings are excluded from the transaction's
                            // balance check. Real and virtual-balanced postings both contribute.
                            // For lot-priced postings, add the *cash* total (in the lot's
                            // commodity) to transaction_state rather than the commodity units.
                            if posting_kind != ast::PostingKind::VirtualUnbalanced {
                                if let Some((lot_total, lot_commodity)) = lot_cash {
                                    let dec = transaction_state.0.entry(lot_commodity).or_default();
                                    *dec += lot_total;
                                } else {
                                    let dec =
                                        transaction_state.0.entry(commodity.clone()).or_default();
                                    *dec += value;
                                }
                            }

                            let by_commodity =
                                BTreeMap::from([(commodity, crate::decimal_to_proto(value))]);
                            resolved_postings.push(crate::elaboration::Posting {
                                account: account_name,
                                payee,
                                amount: Some(crate::elaboration::Amount { by_commodity }),
                                state: crate::state_to_proto(&posting.state.into()),
                                tags: posting.tags,
                                metadata: posting.metadata,
                                kind: crate::posting_kind_to_proto(posting_kind),
                                lot: proto_lot,
                            });
                        } else {
                            // Defer processing and save for next step.
                            // (Null postings are always REAL -- you cannot write a null virtual
                            // posting, so the kind is left as the default Real from the parser.)
                            null_postings.push(posting);
                        }
                    }

                    if null_postings.len() > 1 {
                        return Err(ElaborationError::TooManyNullPostings);
                    }

                    if let Some(mut posting) = null_postings.pop() {
                        let account_name = entry_context
                            .account_aliases
                            .get(&posting.account)
                            .cloned()
                            .unwrap_or(posting.account);
                        let payee = posting.metadata.remove("payee").unwrap_or(payee.clone());

                        // The null posting's amount is the negation of the sum of all
                        // other real/balanced postings (virtual-unbalanced postings are
                        // excluded from transaction_state, so they don't affect inference).
                        // Null postings are always REAL; the kind field is left unspecified
                        // (defaults to 0 = UNSPECIFIED which is treated as REAL by consumers).
                        let by_commodity = transaction_state
                            .0
                            .iter()
                            .map(|(c, v)| (c.clone(), crate::decimal_to_proto(-v)))
                            .collect();

                        resolved_postings.push(crate::elaboration::Posting {
                            account: account_name,
                            payee,
                            amount: Some(crate::elaboration::Amount { by_commodity }),
                            state: crate::state_to_proto(&posting.state.into()),
                            tags: posting.tags,
                            metadata: posting.metadata,
                            kind: crate::posting_kind_to_proto(ast::PostingKind::Real),
                            lot: None,
                        });
                    } else {
                        // Check that transaction state is all zeros to balance the transaction.
                        // Virtual-unbalanced postings have already been excluded from
                        // transaction_state, so a transaction consisting solely of
                        // virtual-unbalanced postings will have an empty (zero) state and
                        // will not trigger this error -- which is the correct ledger-cli behaviour.
                        if transaction_state.0.values().any(|value| !value.is_zero()) {
                            return Err(ElaborationError::TransactionDoesNotBalance(
                                transaction_state,
                            ));
                        }
                    }

                    // Evaluate account-level assert/check directives for each posting.
                    //
                    // `tag()` lookups inherit transaction-level metadata: a
                    // posting with no `; Entity: ...` of its own still sees the
                    // transaction's `Entity` tag (matching OG ledger-cli
                    // semantics). Posting-level metadata wins on key collision.
                    for (posting_index, posting) in resolved_postings.iter().enumerate() {
                        if let Some(props) = value
                            .global_context
                            .account_properties
                            .get(&posting.account)
                        {
                            let merged_metadata =
                                merge_metadata(&transaction.metadata, &posting.metadata);
                            // Assertions and checks operate per-commodity. For
                            // multi-commodity postings each commodity is checked
                            // independently; in practice postings carry a single
                            // commodity.
                            for (commodity, amount_val) in posting.amounts() {
                                for assert_expr in &props.asserts {
                                    let passed = evaluator::eval_bool_expr(
                                        assert_expr,
                                        amount_val,
                                        commodity,
                                        &merged_metadata,
                                        entry_context,
                                        &state,
                                    )
                                    .map_err(ElaborationError::EvaluationError)?;
                                    if !passed {
                                        return Err(ElaborationError::AccountAssertionFailed {
                                            account: posting.account.clone(),
                                            posting_index,
                                            expression: assert_expr.to_string(),
                                        });
                                    }
                                }
                                for check_expr in &props.checks {
                                    let passed = evaluator::eval_bool_expr(
                                        check_expr,
                                        amount_val,
                                        commodity,
                                        &merged_metadata,
                                        entry_context,
                                        &state,
                                    )
                                    .map_err(ElaborationError::EvaluationError)?;
                                    if !passed {
                                        eprintln!(
                                            "warning: check failed for posting {posting_index} \
                                             to {account}: check {expr}",
                                            account = posting.account,
                                            expr = check_expr,
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // Evaluate tag-level assert/check directives.
                    //
                    // Only metadata-style tags (`; TagName: value`) are validated.
                    // Bare colon-tags (e.g. `; :payroll:`) carry no value and are
                    // skipped. Validation applies to both transaction-level metadata
                    // and posting-level metadata for parity with account assertions.

                    // Validate transaction-level metadata tags.
                    eval_tag_metadata(
                        &transaction.metadata,
                        &value.global_context.tag_properties,
                        entry_context,
                        &state,
                    )?;

                    // Validate posting-level metadata tags.
                    for posting in resolved_postings.iter() {
                        eval_tag_metadata(
                            &posting.metadata,
                            &value.global_context.tag_properties,
                            entry_context,
                            &state,
                        )?;
                    }

                    // Update running account balances and register new accounts.
                    // Virtual unbalanced postings DO update the running per-account
                    // balance (matching ledger-cli) so subsequent balance assertions
                    // on the same account see the virtual contribution. They are
                    // excluded only from the transaction-balance check.
                    for posting in resolved_postings.iter() {
                        if !accounts.contains_key(&posting.account) {
                            accounts.insert(posting.account.clone(), Default::default());
                        }

                        let balances = state
                            .account_balances
                            .entry(posting.account.clone())
                            .or_default();
                        for (commodity, delta) in posting.amounts() {
                            *(balances.commodity.entry(commodity.to_string()).or_default()) +=
                                delta;
                        }
                    }

                    transactions.push(crate::elaboration::Transaction {
                        date: transaction.date.to_epoch_days(),
                        secondary_date: transaction.secondary_date.map(|d| d.to_epoch_days()),
                        state: crate::state_to_proto(&transaction.state.into()),
                        code: transaction.code,
                        description: transaction.description,
                        tags: transaction.tags,
                        metadata: transaction.metadata,
                        postings: resolved_postings,
                    });
                }
            }
        }

        // Evaluate each historical price expression using the final (most
        // recent) context, which reflects all directives seen in the file.
        let final_context = value
            .contexts
            .last()
            .expect("HIR always has at least one context");
        let mut prices = vec![];
        for hp in value.prices {
            let (price, price_commodity) =
                evaluator::eval_and_normalize_amount(hp.price, final_context, &state)?;
            prices.push(crate::elaboration::HistoricalPrice {
                date: hp.date.to_epoch_days(),
                time: hp.time,
                commodity: hp.commodity,
                price: Some(crate::decimal_to_proto(price)),
                price_commodity,
            });
        }

        let commodities = value
            .global_context
            .commodity_properties
            .into_iter()
            .map(|(name, p)| {
                (
                    name,
                    crate::elaboration::CommodityProperties {
                        format: p.format,
                        no_market: p.no_market,
                        note: p.note,
                    },
                )
            })
            .collect();

        // Denormalise account metadata by inheritance. For every account
        // in the journal (declared OR only referenced by postings), walk
        // its colon-separated ancestor chain root->leaf, merging in any
        // metadata declared on each ancestor's own `account` directive.
        // Closer ancestors override more distant ones; the account's own
        // declared metadata wins last. Consumers thus see a fully
        // resolved metadata map per account and never need to do the
        // inheritance walk themselves.
        let declared_metadata: BTreeMap<&str, &BTreeMap<String, String>> = value
            .global_context
            .account_properties
            .iter()
            .map(|(name, props)| (name.as_str(), &props.metadata))
            .collect();
        let account_names: Vec<String> = accounts.keys().cloned().collect();
        for name in account_names {
            let mut inherited: BTreeMap<String, String> = BTreeMap::new();
            for prefix in ancestor_prefixes(&name) {
                if let Some(parent_meta) = declared_metadata.get(prefix.as_str()) {
                    for (k, v) in *parent_meta {
                        inherited.insert(k.clone(), v.clone());
                    }
                }
            }
            // `inherited` now holds the merged ancestor metadata in
            // root-->-leaf order (closer wins). For declared accounts
            // their own metadata was the last entry written, so we are
            // already correct. For undeclared accounts (referenced only
            // by postings) `inherited` is purely from ancestors. Either
            // way, overwrite the field.
            if let Some(props) = accounts.get_mut(&name) {
                props.metadata = inherited;
            }
        }

        Ok(crate::elaboration::Journal {
            transactions,
            accounts,
            commodities,
            prices,
        })
    }
}

/// Yield the colon-separated ancestor prefixes of `name`, root first,
/// `name` itself last. So `"Income:Salary:Base"` yields
/// `["Income", "Income:Salary", "Income:Salary:Base"]`.
fn ancestor_prefixes(name: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    for (i, _) in name.match_indices(':') {
        prefixes.push(name[..i].to_string());
    }
    prefixes.push(name.to_string());
    prefixes
}

/// Evaluate tag-level assert/check directives for a set of metadata key-value pairs.
///
/// For each `(tag_name, tag_value)` pair in `metadata`, looks up `tag_name` in
/// Merge transaction-level metadata with posting-level metadata, with the
/// posting's own keys taking precedence. Used when evaluating per-posting
/// `tag()` lookups so that `; Entity: foo` declared at the transaction level
/// is visible to assertions on every posting in that transaction (matching
/// OG ledger-cli semantics).
fn merge_metadata(
    transaction: &BTreeMap<String, String>,
    posting: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = transaction.clone();
    for (k, v) in posting {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// `tag_properties`. If validation rules are found, runs each assert and check
/// with `value` bound to `tag_value` in the expression context.
///
/// - Failed asserts return `Err(ElaborationError::TagAssertionFailed)`.
/// - Failed checks print a warning to stderr but return `Ok(())`.
fn eval_tag_metadata(
    metadata: &BTreeMap<String, String>,
    tag_properties: &BTreeMap<String, resolution::TagProperties>,
    eval_context: &resolution::Context,
    state: &RunningState,
) -> Result<(), ElaborationError> {
    for (tag_name, tag_value) in metadata {
        if let Some(props) = tag_properties.get(tag_name) {
            for assert_expr in &props.asserts {
                let passed =
                    evaluator::eval_bool_expr_for_tag(assert_expr, tag_value, eval_context, state)
                        .map_err(ElaborationError::EvaluationError)?;
                if !passed {
                    return Err(ElaborationError::TagAssertionFailed {
                        tag_name: tag_name.clone(),
                        tag_value: tag_value.clone(),
                        expression: assert_expr.to_string(),
                    });
                }
            }
            for check_expr in &props.checks {
                let passed =
                    evaluator::eval_bool_expr_for_tag(check_expr, tag_value, eval_context, state)
                        .map_err(ElaborationError::EvaluationError)?;
                if !passed {
                    eprintln!(
                        "warning: tag check failed for {tag_name}: \"{tag_value}\": \
                         check {check_expr}",
                    );
                }
            }
        }
    }
    Ok(())
}

/// Expression evaluator: reduces [`ast::ValueExpr`] trees to concrete values.
mod evaluator {
    use std::collections::BTreeMap;

    use regex::Regex;
    use rust_decimal::Decimal;

    use crate::{
        ast::{self, BoolExpr, CmpOp, ValueExpr},
        resolution,
    };

    use super::{ElaborationError, EvaluationError, RunningState};

    /// Evaluate a value expression and extract the `(Decimal, commodity)` pair.
    ///
    /// After evaluation, commodity aliases from `eval_context` are applied so
    /// that e.g. `"Bitcoin"` becomes `"BTC"`. If the result still has no
    /// commodity, the context's `default_commodity` is used. Returns an error
    /// if the result is not an amount or no commodity can be determined.
    pub fn eval_and_normalize_amount(
        val: ast::ValueExpr,
        eval_context: &resolution::Context,
        running_state: &RunningState,
    ) -> Result<(Decimal, String), ElaborationError> {
        eval_and_normalize_amount_with_fallback(val, eval_context, running_state, None)
    }

    /// Like [`eval_and_normalize_amount`], but accepts an optional `fallback_commodity`
    /// that is used when the expression is bare (no commodity) and the context has no
    /// default commodity set. This is used by balance assignments to infer the commodity
    /// from the account's existing running balance.
    pub fn eval_and_normalize_amount_with_fallback(
        val: ast::ValueExpr,
        eval_context: &resolution::Context,
        running_state: &RunningState,
        fallback_commodity: Option<&str>,
    ) -> Result<(Decimal, String), ElaborationError> {
        // Amount expressions don't involve posting metadata (tags); pass an
        // empty map so the `tag()` built-in is a no-op when called from amount
        // contexts (which would be a programmer error, but we don't panic).
        let empty_meta = BTreeMap::default();
        match eval(val, eval_context, running_state, &empty_meta, EVAL_BUDGET)? {
            ast::ValueExpr::Amount { value, commodity } => {
                let commodity = if let Some(commodity) = commodity {
                    // Apply commodity alias (e.g. "Bitcoin" -> "BTC")
                    eval_context
                        .commodity_aliases
                        .get(&commodity)
                        .unwrap_or(&commodity)
                        .clone()
                } else {
                    // No commodity in the expression -- try context default, then
                    // the caller-supplied fallback (e.g. inferred from account balance).
                    eval_context
                        .default_commodity
                        .as_deref()
                        .or(fallback_commodity)
                        .ok_or(ElaborationError::AmountWithNoCommodity)?
                        .to_owned()
                };
                Ok((value, commodity))
            }
            val => Err(ElaborationError::NonAmountWhereAmountExpected(val)),
        }
    }

    /// Evaluate a [`BoolExpr`] in the context of a posting.
    ///
    /// `posting_amount` and `posting_commodity` are bound as `amount` and
    /// `commodity` in the expression context, which is how account assertions
    /// refer to the current posting's values.
    ///
    /// `posting_metadata` provides the key-value tag pairs from the posting's
    /// notes (e.g. `; Entity: Foo` -> `{"Entity": "Foo"}`). This is used by
    /// the `tag("name")` built-in to look up metadata values.
    ///
    /// Returns `true` if the assertion passes, `false` if it fails, or an
    /// error if expression evaluation itself fails.
    pub fn eval_bool_expr(
        expr: &BoolExpr,
        posting_amount: Decimal,
        posting_commodity: &str,
        posting_metadata: &BTreeMap<String, String>,
        eval_context: &resolution::Context,
        state: &RunningState,
    ) -> Result<bool, EvaluationError> {
        // Build a temporary context with `amount` and `commodity` injected as
        // zero-parameter defines so the evaluator can resolve them.
        let mut ctx = eval_context.clone();
        ctx.defines.insert(
            "amount".into(),
            resolution::Define {
                params: vec![],
                body: ast::DefineBody::Value(ast::ValueExpr::Amount {
                    value: posting_amount,
                    commodity: Some(posting_commodity.to_string()),
                }),
            },
        );
        ctx.defines.insert(
            "commodity".into(),
            resolution::Define {
                params: vec![],
                body: ast::DefineBody::Value(ast::ValueExpr::Str(posting_commodity.to_string())),
            },
        );

        // Check whether the LHS is a call to a bool-body define. If it is and
        // there is no comparison operator, we expand the define body as a full
        // bool expression rather than treating the call result as a numeric value.
        if expr.cmp.is_none()
            && let ast::ValueExpr::Function { name, args } = &expr.lhs
            && let Some(define) = ctx.defines.get(name.as_str())
            && let ast::DefineBody::Bool(body) = define.body.clone()
        {
            if define.params.len() != args.len() {
                return Err(EvaluationError::DefineArgCountMismatch {
                    name: name.clone(),
                    expected: define.params.len(),
                    got: args.len(),
                });
            }
            // Bind arguments into a new context and evaluate the bool body.
            let mut call_ctx = ctx.clone();
            for (param, arg_expr) in define.params.iter().zip(args.iter()) {
                let arg_val = eval(arg_expr.clone(), &ctx, state, posting_metadata, EVAL_BUDGET)?;
                call_ctx.defines.insert(
                    param.clone(),
                    resolution::Define {
                        params: vec![],
                        body: ast::DefineBody::Value(arg_val),
                    },
                );
            }
            // Evaluate the define's bool body, then apply any chain.
            let segment_result = eval_bool_expr_with_context(
                &body,
                posting_amount,
                posting_commodity,
                posting_metadata,
                &call_ctx,
                state,
            )?;
            return match &expr.chain {
                None => Ok(segment_result),
                Some((ast::BoolOp::And, cont)) => {
                    if !segment_result {
                        Ok(false)
                    } else {
                        eval_bool_expr(
                            cont,
                            posting_amount,
                            posting_commodity,
                            posting_metadata,
                            eval_context,
                            state,
                        )
                    }
                }
                Some((ast::BoolOp::Or, cont)) => {
                    if segment_result {
                        Ok(true)
                    } else {
                        eval_bool_expr(
                            cont,
                            posting_amount,
                            posting_commodity,
                            posting_metadata,
                            eval_context,
                            state,
                        )
                    }
                }
            };
        }

        // Evaluate LHS.
        let lhs_val = eval(expr.lhs.clone(), &ctx, state, posting_metadata, EVAL_BUDGET)?;

        // Compute this segment's boolean value. With no comparison operator,
        // the expression is truthy iff the LHS evaluates to a non-zero amount
        // (unusual but consistent). Either way, an `expr.chain` continuation
        // must still be evaluated below.
        let result = match &expr.cmp {
            None => match lhs_val {
                ast::ValueExpr::Amount { value, .. } => !value.is_zero(),
                _ => false,
            },
            Some((cmp_op, rhs_expr)) => {
                // For regex comparisons the RHS is already a Regex literal in
                // the AST -- pass it through to eval_cmp without re-evaluating.
                let rhs_val = match rhs_expr {
                    ast::ValueExpr::Regex(_) => rhs_expr.clone(),
                    other => eval(other.clone(), &ctx, state, posting_metadata, EVAL_BUDGET)?,
                };
                eval_cmp(cmp_op, &lhs_val, &rhs_val)?
            }
        };

        // If there is a boolean chain, short-circuit accordingly.
        match &expr.chain {
            None => Ok(result),
            Some((ast::BoolOp::And, cont)) => {
                if !result {
                    Ok(false)
                } else {
                    eval_bool_expr(
                        cont,
                        posting_amount,
                        posting_commodity,
                        posting_metadata,
                        eval_context,
                        state,
                    )
                }
            }
            Some((ast::BoolOp::Or, cont)) => {
                if result {
                    Ok(true)
                } else {
                    eval_bool_expr(
                        cont,
                        posting_amount,
                        posting_commodity,
                        posting_metadata,
                        eval_context,
                        state,
                    )
                }
            }
        }
    }

    /// Evaluate a [`BoolExpr`] using a pre-built context that already has
    /// parameter bindings in place.
    ///
    /// This is the inner workhorse used when expanding a bool-body define call:
    /// the caller has already bound the define's parameters as zero-param value
    /// defines in `eval_context`, so we evaluate the body without re-injecting
    /// `amount`/`commodity` (they are already in the context or in `eval_context`).
    // posting_amount and posting_commodity are forwarded through recursive calls
    // to eval_bool_expr (for chains); they are not used directly in the body.
    #[allow(clippy::only_used_in_recursion)]
    fn eval_bool_expr_with_context(
        expr: &BoolExpr,
        posting_amount: Decimal,
        posting_commodity: &str,
        posting_metadata: &BTreeMap<String, String>,
        eval_context: &resolution::Context,
        state: &RunningState,
    ) -> Result<bool, EvaluationError> {
        // Check for a bool-body define call in the LHS (recursive define calls).
        if expr.cmp.is_none()
            && let ast::ValueExpr::Function { name, args } = &expr.lhs
            && let Some(define) = eval_context.defines.get(name.as_str())
            && let ast::DefineBody::Bool(body) = define.body.clone()
        {
            if define.params.len() != args.len() {
                return Err(EvaluationError::DefineArgCountMismatch {
                    name: name.clone(),
                    expected: define.params.len(),
                    got: args.len(),
                });
            }
            let mut call_ctx = eval_context.clone();
            for (param, arg_expr) in define.params.iter().zip(args.iter()) {
                let arg_val = eval(
                    arg_expr.clone(),
                    eval_context,
                    state,
                    posting_metadata,
                    EVAL_BUDGET,
                )?;
                call_ctx.defines.insert(
                    param.clone(),
                    resolution::Define {
                        params: vec![],
                        body: ast::DefineBody::Value(arg_val),
                    },
                );
            }
            let segment_result = eval_bool_expr_with_context(
                &body,
                posting_amount,
                posting_commodity,
                posting_metadata,
                &call_ctx,
                state,
            )?;
            return match &expr.chain {
                None => Ok(segment_result),
                Some((ast::BoolOp::And, cont)) => {
                    if !segment_result {
                        Ok(false)
                    } else {
                        eval_bool_expr_with_context(
                            cont,
                            posting_amount,
                            posting_commodity,
                            posting_metadata,
                            eval_context,
                            state,
                        )
                    }
                }
                Some((ast::BoolOp::Or, cont)) => {
                    if segment_result {
                        Ok(true)
                    } else {
                        eval_bool_expr_with_context(
                            cont,
                            posting_amount,
                            posting_commodity,
                            posting_metadata,
                            eval_context,
                            state,
                        )
                    }
                }
            };
        }

        let lhs_val = eval(
            expr.lhs.clone(),
            eval_context,
            state,
            posting_metadata,
            EVAL_BUDGET,
        )?;
        let result = match &expr.cmp {
            None => match lhs_val {
                ast::ValueExpr::Amount { value, .. } => !value.is_zero(),
                _ => false,
            },
            Some((cmp_op, rhs_expr)) => {
                let rhs_val = match rhs_expr {
                    ast::ValueExpr::Regex(_) => rhs_expr.clone(),
                    other => eval(
                        other.clone(),
                        eval_context,
                        state,
                        posting_metadata,
                        EVAL_BUDGET,
                    )?,
                };
                eval_cmp(cmp_op, &lhs_val, &rhs_val)?
            }
        };

        match &expr.chain {
            None => Ok(result),
            Some((ast::BoolOp::And, cont)) => {
                if !result {
                    Ok(false)
                } else {
                    eval_bool_expr_with_context(
                        cont,
                        posting_amount,
                        posting_commodity,
                        posting_metadata,
                        eval_context,
                        state,
                    )
                }
            }
            Some((ast::BoolOp::Or, cont)) => {
                if result {
                    Ok(true)
                } else {
                    eval_bool_expr_with_context(
                        cont,
                        posting_amount,
                        posting_commodity,
                        posting_metadata,
                        eval_context,
                        state,
                    )
                }
            }
        }
    }

    /// Evaluate a [`BoolExpr`] in the context of a tag metadata value.
    ///
    /// `tag_value` is bound as `value` (a `Str`) in the expression context.
    /// This is the mechanism used by `tag` directive `assert`/`check` bodies
    /// to refer to the metadata value, e.g. `value =~ /^foo/`.
    ///
    /// Tag assertions have no associated posting amount or commodity, so those
    /// bindings are not injected. The `tag()` built-in is available but looks
    /// up from an empty metadata map (tag assertions are about the tag value
    /// itself, not about other metadata on the same entry).
    pub fn eval_bool_expr_for_tag(
        expr: &BoolExpr,
        tag_value: &str,
        eval_context: &resolution::Context,
        state: &RunningState,
    ) -> Result<bool, EvaluationError> {
        let empty_meta = BTreeMap::default();

        // Inject `value` as a Str binding so the expression can reference it.
        let mut ctx = eval_context.clone();
        ctx.defines.insert(
            "value".into(),
            resolution::Define {
                params: vec![],
                body: ast::DefineBody::Value(ast::ValueExpr::Str(tag_value.to_string())),
            },
        );

        let lhs_val = eval(expr.lhs.clone(), &ctx, state, &empty_meta, EVAL_BUDGET)?;

        let result = match &expr.cmp {
            None => match lhs_val {
                ast::ValueExpr::Amount { value, .. } => !value.is_zero(),
                _ => false,
            },
            Some((cmp_op, rhs_expr)) => {
                let rhs_val = match rhs_expr {
                    ast::ValueExpr::Regex(_) => rhs_expr.clone(),
                    other => eval(other.clone(), &ctx, state, &empty_meta, EVAL_BUDGET)?,
                };
                eval_cmp(cmp_op, &lhs_val, &rhs_val)?
            }
        };

        match &expr.chain {
            None => Ok(result),
            Some((ast::BoolOp::And, cont)) => {
                if !result {
                    Ok(false)
                } else {
                    eval_bool_expr_for_tag(cont, tag_value, eval_context, state)
                }
            }
            Some((ast::BoolOp::Or, cont)) => {
                if result {
                    Ok(true)
                } else {
                    eval_bool_expr_for_tag(cont, tag_value, eval_context, state)
                }
            }
        }
    }

    /// Compare two evaluated [`ast::ValueExpr`] values with a [`CmpOp`].
    ///
    /// Supported comparisons:
    /// - `Str == Str` / `Str != Str` -- commodity identity checks
    /// - `Str =~ Regex` / `Str !~ Regex` -- regex match against a string
    /// - `Amount cmp Amount` -- numeric comparisons (same or compatible commodities)
    ///
    /// Regex matching is case-sensitive by default (Rust `regex` crate semantics).
    /// Returns an error for type mismatches or an invalid regex pattern.
    fn eval_cmp(
        op: &CmpOp,
        lhs: &ast::ValueExpr,
        rhs: &ast::ValueExpr,
    ) -> Result<bool, EvaluationError> {
        match (lhs, rhs) {
            // Regex match: LHS must be a string, RHS must be a Regex literal.
            // Patterns are validated at parse time, so compilation here cannot
            // fail in well-formed input.
            (ast::ValueExpr::Str(text), ast::ValueExpr::Regex(pattern)) => {
                let re = Regex::new(pattern).map_err(|e| {
                    EvaluationError::InvalidRegexPattern(pattern.clone(), e.to_string())
                })?;
                // The only CmpOps valid with a Regex RHS are RegexMatch and
                // RegexNotMatch -- any other combination is a parser-level bug.
                Ok(match op {
                    CmpOp::RegexMatch => re.is_match(text),
                    CmpOp::RegexNotMatch => !re.is_match(text),
                    _ => unreachable!(
                        "parser should only produce RegexMatch/RegexNotMatch with a Regex RHS"
                    ),
                })
            }
            // String equality: used for `commodity == "$"`.
            (ast::ValueExpr::Str(a), ast::ValueExpr::Str(b)) => Ok(match op {
                CmpOp::Eq => a == b,
                CmpOp::Ne => a != b,
                _ => {
                    return Err(EvaluationError::BinaryOperationTypeError((
                        lhs.clone(),
                        rhs.clone(),
                        ast::Op::Add, // placeholder op; no ordering on strings
                    )));
                }
            }),
            // Numeric comparisons: used for `amount > 0`, etc.
            (
                ast::ValueExpr::Amount {
                    value: v1,
                    commodity: c1,
                },
                ast::ValueExpr::Amount {
                    value: v2,
                    commodity: c2,
                },
            ) if c1 == c2 || c1.is_none() || c2.is_none() => Ok(match op {
                CmpOp::Eq => v1 == v2,
                CmpOp::Ne => v1 != v2,
                CmpOp::Lt => v1 < v2,
                CmpOp::Le => v1 <= v2,
                CmpOp::Gt => v1 > v2,
                CmpOp::Ge => v1 >= v2,
                // Regex operators on amounts are a type error.
                CmpOp::RegexMatch | CmpOp::RegexNotMatch => {
                    return Err(EvaluationError::BinaryOperationTypeError((
                        lhs.clone(),
                        rhs.clone(),
                        ast::Op::Add,
                    )));
                }
            }),
            _ => Err(EvaluationError::BinaryOperationTypeError((
                lhs.clone(),
                rhs.clone(),
                ast::Op::Add,
            ))),
        }
    }

    /// Recursively evaluate a [`ast::ValueExpr`] to a simpler form.
    ///
    /// The evaluator reduces arithmetic, applies unary operators, resolves
    /// function calls, and handles type annotations. It does not resolve
    /// commodity aliases -- that is done by `eval_and_normalize_amount`.
    ///
    /// `posting_metadata` carries the key-value tag pairs from the posting's
    /// notes. It is forwarded into recursive calls and is read by the `tag()`
    /// built-in function.
    /// Initial recursion budget passed to [`eval`] by every external caller.
    /// Each recursive `eval` call decrements `budget`; `eval` errors with
    /// [`EvaluationError::RecursionLimitExceeded`] when it reaches 0. Protects
    /// against cyclic `define`s (e.g. `define a = b; define b = a`), which
    /// would otherwise recurse until the OS aborts the process with a stack
    /// overflow.
    ///
    /// 64 is well above any sane real-world expression depth and well below
    /// debug-build stack limits (debug frames are large).
    pub const EVAL_BUDGET: usize = 64;

    fn eval(
        val: ast::ValueExpr,
        eval_context: &resolution::Context,
        state: &RunningState,
        posting_metadata: &BTreeMap<String, String>,
        budget: usize,
    ) -> Result<ast::ValueExpr, EvaluationError> {
        let Some(budget) = budget.checked_sub(1) else {
            return Err(EvaluationError::RecursionLimitExceeded);
        };
        match val {
            // Base cases: already-reduced values pass through unchanged.
            a @ ast::ValueExpr::Amount { .. } => Ok(a),
            s @ ast::ValueExpr::Str(_) => Ok(s),
            r @ ast::ValueExpr::Regex(_) => Ok(r),
            o @ ast::ValueExpr::Object(_) => Ok(o),

            ast::ValueExpr::Unary { op, expr } => {
                match eval(*expr, eval_context, state, posting_metadata, budget)? {
                    ast::ValueExpr::Amount { value, commodity } => match op {
                        ast::Op::Sub => Ok(ast::ValueExpr::Amount {
                            value: -value,
                            commodity,
                        }),
                        ast::Op::Add => Ok(ast::ValueExpr::Amount { value, commodity }),
                        // Unary * and / are not defined for amounts.
                        _ => Err(EvaluationError::UnaryMultiplyOrDivide),
                    },
                    val => Err(EvaluationError::UnaryOnNonAmount(val)),
                }
            }

            ast::ValueExpr::Binary { lhs, rhs, op } => {
                match (
                    eval(*lhs, eval_context, state, posting_metadata, budget)?,
                    eval(*rhs, eval_context, state, posting_metadata, budget)?,
                ) {
                    // One side has a commodity, the other is dimensionless --
                    // the commodity propagates to the result. Both match arms
                    // handle the two orderings (commodity first or second).
                    (
                        ast::ValueExpr::Amount {
                            value: v1,
                            commodity: c,
                        },
                        ast::ValueExpr::Amount {
                            value: v2,
                            commodity: None,
                        },
                    )
                    | (
                        ast::ValueExpr::Amount {
                            value: v1,
                            commodity: None,
                        },
                        ast::ValueExpr::Amount {
                            value: v2,
                            commodity: c,
                        },
                    ) => Ok(match op {
                        ast::Op::Add => ast::ValueExpr::Amount {
                            value: v1 + v2,
                            commodity: c,
                        },
                        ast::Op::Sub => ast::ValueExpr::Amount {
                            value: v1 - v2,
                            commodity: c,
                        },
                        ast::Op::Mul => ast::ValueExpr::Amount {
                            value: v1 * v2,
                            commodity: c,
                        },
                        ast::Op::Div => ast::ValueExpr::Amount {
                            value: v1 / v2,
                            commodity: c,
                        },
                    }),

                    // Both sides have the same commodity -- straightforward arithmetic.
                    (
                        ast::ValueExpr::Amount {
                            value: v1,
                            commodity: c,
                        },
                        ast::ValueExpr::Amount {
                            value: v2,
                            commodity: c2,
                        },
                    ) if c == c2 => Ok(match op {
                        ast::Op::Add => ast::ValueExpr::Amount {
                            value: v1 + v2,
                            commodity: c,
                        },
                        ast::Op::Sub => ast::ValueExpr::Amount {
                            value: v1 - v2,
                            commodity: c,
                        },
                        ast::Op::Mul => ast::ValueExpr::Amount {
                            value: v1 * v2,
                            commodity: c,
                        },
                        ast::Op::Div => ast::ValueExpr::Amount {
                            value: v1 / v2,
                            commodity: c,
                        },
                    }),

                    // Special case: "$-123" parses as Commodity("$") sub Amount(123, None).
                    // The grammar sees the minus sign as a binary subtraction between the
                    // currency symbol and the following number because of how prefix_op and
                    // the amount rule interact. We handle it by treating Sub as negation.
                    (
                        ast::ValueExpr::Commodity(commodity),
                        ast::ValueExpr::Amount {
                            value,
                            commodity: None,
                        },
                    )
                    | (
                        ast::ValueExpr::Amount {
                            value,
                            commodity: None,
                        },
                        ast::ValueExpr::Commodity(commodity),
                    ) => match op {
                        ast::Op::Sub => Ok(ast::ValueExpr::Amount {
                            value: -value,
                            commodity: Some(commodity),
                        }),
                        ast::Op::Add => Ok(ast::ValueExpr::Amount {
                            value,
                            commodity: Some(commodity),
                        }),
                        _ => Err(EvaluationError::UnaryMultiplyOrDivide),
                    },

                    (a, b) => Err(EvaluationError::BinaryOperationTypeError((a, b, op))),
                }
            }

            ast::ValueExpr::Function { name, args } => {
                // Check user-defined parameterized macros before built-ins.
                if let Some(define) = eval_context.defines.get(name.as_str()) {
                    if define.params.len() != args.len() {
                        return Err(EvaluationError::DefineArgCountMismatch {
                            name: name.clone(),
                            expected: define.params.len(),
                            got: args.len(),
                        });
                    }
                    return match &define.body {
                        ast::DefineBody::Bool(_) => {
                            Err(EvaluationError::BoolDefineInValueContext(name.clone()))
                        }
                        ast::DefineBody::Value(body_expr) => {
                            // Evaluate arguments in the caller's context, then
                            // bind them by name in a temporary child context.
                            let mut ctx = eval_context.clone();
                            for (param, arg_expr) in define.params.iter().zip(args.iter()) {
                                let arg_val = eval(
                                    arg_expr.clone(),
                                    eval_context,
                                    state,
                                    posting_metadata,
                                    budget,
                                )?;
                                ctx.defines.insert(
                                    param.clone(),
                                    resolution::Define {
                                        params: vec![],
                                        body: ast::DefineBody::Value(arg_val),
                                    },
                                );
                            }
                            eval(body_expr.clone(), &ctx, state, posting_metadata, budget)
                        }
                    };
                }

                match (name.as_str(), args.as_slice()) {
                    // scrub(x) -- identity function used by some ledger-cli extensions
                    // to mark amounts as "scrubbed" (processed). Treated as a no-op.
                    ("scrub", [arg]) => {
                        eval(arg.clone(), eval_context, state, posting_metadata, budget)
                    }

                    // account("Name") -- returns an object with a "total" field
                    // containing the current running balance of the named account.
                    // Only the primary commodity ($) is currently surfaced.
                    ("account", [account]) => {
                        if let ValueExpr::Str(account) = eval(
                            account.clone(),
                            eval_context,
                            state,
                            posting_metadata,
                            budget,
                        )? {
                            let account = eval_context
                                .account_aliases
                                .get(&account)
                                .unwrap_or(&account);
                            let balance = state
                                .account_balances
                                .get(account)
                                .and_then(|ab| ab.commodity.get("$"))
                                .cloned()
                                .unwrap_or_default();
                            Ok(ast::ValueExpr::Object(BTreeMap::from([(
                                "total".into(),
                                ast::ValueExpr::Amount {
                                    value: balance,
                                    commodity: Some("$".into()),
                                },
                            )])))
                        } else {
                            Err(EvaluationError::InvalidFunctionArgs((
                                name,
                                account.clone(),
                            )))
                        }
                    }

                    // tag("name") -- looks up a metadata key on the current posting.
                    //
                    // The posting's notes are parsed into key-value pairs by the
                    // resolution stage (e.g. `; Entity: Foo` -> `{"Entity": "Foo"}`).
                    // If the key is present its value is returned as a Str; if absent
                    // an empty string is returned so that `tag("X") =~ /pattern/`
                    // works naturally (empty string never matches a non-empty pattern).
                    ("tag", [key_expr]) => {
                        if let ValueExpr::Str(key) = eval(
                            key_expr.clone(),
                            eval_context,
                            state,
                            posting_metadata,
                            budget,
                        )? {
                            let value = posting_metadata.get(&key).cloned().unwrap_or_default();
                            Ok(ast::ValueExpr::Str(value))
                        } else {
                            Err(EvaluationError::InvalidFunctionArgs((
                                name,
                                key_expr.clone(),
                            )))
                        }
                    }

                    _ => Err(EvaluationError::UnknownFunctionArgs((name, args))),
                }
            }

            // A bare commodity symbol or identifier. If the name matches a
            // `define` alias in the active context, substitute and re-evaluate
            // the stored expression. Otherwise return the Commodity as-is;
            // the Binary handler above resolves it when paired with a number.
            ast::ValueExpr::Commodity(ref name) => {
                if let Some(define) = eval_context.defines.get(name.as_str()) {
                    // Zero-parameter defines act as simple aliases: expand the body.
                    // Parameterized defines require a call site (handled in the
                    // Function arm above); a bare reference is not valid.
                    if define.params.is_empty() {
                        match &define.body {
                            ast::DefineBody::Value(expr) => {
                                eval(expr.clone(), eval_context, state, posting_metadata, budget)
                            }
                            // A boolean-body define referenced as a plain identifier is
                            // not meaningful in a value-expression context; treat it as
                            // an unresolved commodity (same as if it were undefined).
                            ast::DefineBody::Bool(_) => Ok(val),
                        }
                    } else {
                        Ok(val)
                    }
                } else {
                    Ok(val)
                }
            }

            ast::ValueExpr::Typed {
                expr,
                commodity: new_commodity,
            } => match eval(*expr, eval_context, state, posting_metadata, budget)? {
                // Accept if the inner expression has no commodity or the same commodity.
                ast::ValueExpr::Amount { value, commodity }
                    if commodity.is_none() || commodity.as_ref() == Some(&new_commodity) =>
                {
                    Ok(ast::ValueExpr::Amount {
                        value,
                        commodity: Some(new_commodity),
                    })
                }
                a => Err(EvaluationError::TypedCommodityToIncompatibleAmount((
                    new_commodity,
                    a,
                ))),
            },

            ast::ValueExpr::Access { expr, field } => {
                match eval(*expr, eval_context, state, posting_metadata, budget)? {
                    ast::ValueExpr::Object(map) => map
                        .get(&field)
                        .cloned()
                        .ok_or(EvaluationError::NoSuchField(field)),
                    val => Err(EvaluationError::FieldAccessTypeError(val)),
                }
            }

            // A parenthesised boolean expression in a value-expression position,
            // e.g. `(amt > 0 or tag("TaxImplication") !~ /^\s*$/)`.
            //
            // Evaluate the inner [`ast::BoolExpr`] and convert the result to a
            // dimensionless `Amount` of `1` (true) or `0` (false) so it can
            // participate in arithmetic or be used as the LHS of a comparison.
            //
            // `posting_amount`/`posting_commodity` are extracted from the
            // defines that `eval_bool_expr` injects before calling `eval`;
            // if absent (Group used outside a posting context) we default to 0/"".
            ast::ValueExpr::Group(bool_expr) => {
                let (posting_amount, posting_commodity) =
                    extract_posting_context_from_defines(eval_context);
                let result = eval_bool_expr_with_context(
                    &bool_expr,
                    posting_amount,
                    &posting_commodity,
                    posting_metadata,
                    eval_context,
                    state,
                )?;
                Ok(ast::ValueExpr::Amount {
                    value: if result { Decimal::ONE } else { Decimal::ZERO },
                    commodity: None,
                })
            }
        }
    }

    /// Extract posting amount and commodity from the defines that
    /// [`eval_bool_expr`] injects into the eval context before calling [`eval`].
    ///
    /// Returns `(Decimal::ZERO, "")` when the bindings are absent, which
    /// happens when a `Group` expression appears outside a posting context.
    fn extract_posting_context_from_defines(ctx: &resolution::Context) -> (Decimal, String) {
        let amount = ctx
            .defines
            .get("amount")
            .and_then(|d| {
                if let ast::DefineBody::Value(ast::ValueExpr::Amount { value, .. }) = &d.body {
                    Some(*value)
                } else {
                    None
                }
            })
            .unwrap_or(Decimal::ZERO);
        let commodity = ctx
            .defines
            .get("commodity")
            .and_then(|d| {
                if let ast::DefineBody::Value(ast::ValueExpr::Str(s)) = &d.body {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        (amount, commodity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn test_amount_default_is_empty() {
        let amount = Amount::default();
        assert!(amount.0.is_empty());
    }

    #[test]
    fn test_amount_multi_commodity() {
        let amount = Amount(BTreeMap::from([
            ("USD".to_string(), dec!(42.5)),
            ("$".to_string(), dec!(-1.5)),
        ]));
        assert_eq!(amount.0.len(), 2);
        assert_eq!(amount.0["USD"], dec!(42.5));
        assert_eq!(amount.0["$"], dec!(-1.5));
    }

    #[test]
    fn test_prices_wired_through_to_journal() {
        use crate::{ast, resolution};

        // Build an AST journal with a single P directive.
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
        let journal_ast = ast::Journal {
            entries: vec![ast::Entry::HistoricalPrice(price_ast)],
        };

        // Resolution stage.
        let hir = resolution::HIR::try_from(journal_ast).expect("resolution should succeed");
        assert_eq!(hir.prices.len(), 1, "HIR should contain one price");

        // Elaboration stage.
        let journal =
            crate::elaboration::Journal::try_from(hir).expect("elaboration should succeed");

        assert_eq!(journal.prices.len(), 1, "Journal should contain one price");
        let price = &journal.prices[0];

        // date: 2024-06-15 -> days since epoch
        let expected_days = chrono::NaiveDate::from_ymd_opt(2024, 6, 15)
            .unwrap()
            .to_epoch_days();
        assert_eq!(price.date, expected_days);
        assert_eq!(price.time.as_deref(), Some("14:30:00"));
        assert_eq!(price.commodity, "AAPL");
        assert_eq!(
            price.price.as_ref().unwrap().to_decimal(),
            rust_decimal::Decimal::from(182)
        );
        assert_eq!(price.price_commodity, "$");
    }

    /// Parse a ledger journal string through the full pipeline and return the
    /// elaborated `Journal`. Panics on any parse/resolution/elaboration error.
    fn elaborate(input: &str) -> crate::elaboration::Journal {
        use crate::{grammars::ledger::parse_ledger, resolution::HIR};
        let ast = parse_ledger(input).expect("parse failed");
        let hir = HIR::try_from(ast).expect("resolution failed");
        crate::elaboration::Journal::try_from(hir).expect("elaboration failed")
    }

    #[test]
    fn test_define_simple_amount_alias() {
        // `monthly_rent` is defined as $1500.00 and used in a posting amount.
        let input = "\
define monthly_rent = $1500.00

2024-01-01 Rent Payment
    Expenses:Rent  monthly_rent
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
        let tx = &journal.transactions[0];
        // The Expenses:Rent posting should have $1500.00
        let rent_posting = tx
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Rent")
            .unwrap();
        assert_eq!(
            rent_posting.amount_in("$"),
            Some(dec!(1500.00)),
            "define alias should expand to $1500.00"
        );
        // Assets:Checking should be the balancing null posting: -$1500.00
        let checking_posting = tx
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking")
            .unwrap();
        assert_eq!(
            checking_posting.amount_in("$"),
            Some(dec!(-1500.00)),
            "balancing posting should be -$1500.00"
        );
    }

    #[test]
    fn test_define_used_in_arithmetic_expression() {
        // Aliases can appear inside arithmetic: `2 * base_amount`.
        let input = "\
define base_amount = 100 USD

2024-02-01 Double Amount
    Expenses:Food  2 * base_amount
    Assets:Cash
";
        // The expression `2 * base_amount` becomes `2 * 100 USD` = `200 USD`.
        // Note: `base_amount` parses as Commodity("base_amount"); after define
        // substitution it becomes Amount{100, Some("USD")}.
        // `2` parses as Amount{2, None}. Mul of (None, USD) -> 200 USD.
        let journal = elaborate(input);
        let tx = &journal.transactions[0];
        let food = tx
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(
            food.amount_in("USD"),
            Some(dec!(200)),
            "2 * define alias should expand to 200 USD"
        );
    }

    #[test]
    fn test_define_does_not_affect_earlier_transactions() {
        // A define directive must not retroactively affect transactions that
        // appeared before it in the source file.
        //
        // We test this by parsing a transaction where `myval` is NOT yet
        // defined -- it should be treated as a bare commodity rather than an
        // alias, causing an evaluation error (non-amount commodity alone does
        // not balance). The transaction after the define succeeds.
        //
        // Actually: a bare Commodity alone won't resolve to an amount, so the
        // first transaction would fail to elaborate. Instead, use an explicit
        // amount for the first transaction and verify the define is only in
        // context 1 via the HIR, not context 0.
        use crate::{grammars::ledger::parse_ledger, resolution::HIR};

        let input = "\
2024-01-01 Before Define
    Expenses:A  $10.00
    Assets:Cash

define myval = $99.00

2024-01-02 After Define
    Expenses:B  myval
    Assets:Cash
";
        let ast = parse_ledger(input).expect("parse failed");
        let hir = HIR::try_from(ast).expect("resolution failed");

        // There should be 2 contexts (0 = initial, 1 = after define).
        assert_eq!(hir.contexts.len(), 2);
        // First transaction references context 0 -- no defines.
        assert_eq!(hir.entries[0].context_id, 0);
        assert!(hir.contexts[0].defines.is_empty());
        // Second transaction references context 1 -- has the define.
        assert_eq!(hir.entries[1].context_id, 1);
        assert!(hir.contexts[1].defines.contains_key("myval"));

        // Elaboration should succeed end-to-end.
        let journal = crate::elaboration::Journal::try_from(hir).expect("elaboration failed");
        let after_tx = &journal.transactions[1];
        let b_posting = after_tx
            .postings
            .iter()
            .find(|p| p.account == "Expenses:B")
            .unwrap();
        assert_eq!(b_posting.amount_in("$"), Some(dec!(99.00)));
    }

    // -----------------------------------------------------------------------
    // Tests for `--cleared` flag: TransactionState threading and filtering
    // -----------------------------------------------------------------------

    /// Verify that transaction state (`*` cleared, no marker uncleared) is
    /// preserved all the way through parse -> resolution -> elaboration.
    #[test]
    fn test_transaction_state_preserved_through_pipeline() {
        let input = "\
2024-01-01 * Cleared Transaction
    Expenses:Food  $10.00
    Assets:Checking

2024-01-02 Uncleared Transaction
    Expenses:Food  $5.00
    Assets:Checking

2024-01-03 ! Pending Transaction
    Expenses:Food  $3.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 3);
        assert!(
            journal.transactions[0].state == crate::elaboration::TransactionState::Cleared as i32,
            "first transaction should be Cleared"
        );
        assert!(
            journal.transactions[1].state == crate::elaboration::TransactionState::Uncleared as i32,
            "second transaction should be Uncleared"
        );
        assert!(
            journal.transactions[2].state == crate::elaboration::TransactionState::Pending as i32,
            "third transaction should be Pending"
        );
    }

    /// Simulate the `balance --cleared` filter: only transactions with state
    /// `Cleared` should contribute to balances.
    ///
    /// Mixed input: one cleared ($10), one uncleared ($5), one pending ($3).
    /// The filtered balance for `Expenses:Food` should be $10 only.
    #[test]
    fn test_cleared_filter_mixed_transactions() {
        let input = "\
2024-01-01 * Cleared Transaction
    Expenses:Food  $10.00
    Assets:Checking

2024-01-02 Uncleared Transaction
    Expenses:Food  $5.00
    Assets:Checking

2024-01-03 ! Pending Transaction
    Expenses:Food  $3.00
    Assets:Checking
";
        let journal = elaborate(input);

        // Reproduce the `--cleared` filter from main.rs.
        let cleared_total: rust_decimal::Decimal = journal
            .transactions
            .iter()
            .filter(|txn| txn.state == crate::elaboration::TransactionState::Cleared as i32)
            .flat_map(|txn| txn.postings.iter())
            .filter(|p| p.account == "Expenses:Food")
            .filter_map(|p| p.amount_in("$"))
            .sum();

        assert_eq!(
            cleared_total,
            dec!(10.00),
            "--cleared should include only the $10.00 cleared transaction"
        );
    }

    /// When no transactions are cleared, filtering by `--cleared` yields an
    /// empty result (no contributions to any account balance).
    #[test]
    fn test_cleared_filter_no_cleared_transactions() {
        let input = "\
2024-01-01 Uncleared One
    Expenses:Food  $10.00
    Assets:Checking

2024-01-02 ! Pending One
    Expenses:Food  $5.00
    Assets:Checking
";
        let journal = elaborate(input);

        let count = journal
            .transactions
            .iter()
            .filter(|txn| txn.state == crate::elaboration::TransactionState::Cleared as i32)
            .count();

        assert_eq!(count, 0, "no cleared transactions should be found");
    }

    /// Without `--cleared`, all transactions (cleared and uncleared) are
    /// included in the balance. Verifies the default behaviour is unchanged.
    #[test]
    fn test_no_cleared_filter_includes_all_transactions() {
        let input = "\
2024-01-01 * Cleared Transaction
    Expenses:Food  $10.00
    Assets:Checking

2024-01-02 Uncleared Transaction
    Expenses:Food  $5.00
    Assets:Checking
";
        let journal = elaborate(input);

        // No filter -- sum all transactions.
        let total: rust_decimal::Decimal = journal
            .transactions
            .iter()
            .flat_map(|txn| txn.postings.iter())
            .filter(|p| p.account == "Expenses:Food")
            .filter_map(|p| p.amount_in("$"))
            .sum();

        assert_eq!(
            total,
            dec!(15.00),
            "without --cleared both transactions should contribute to the balance"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for standalone balance assertion enforcement (issue #37)
    // -----------------------------------------------------------------------

    /// Try to elaborate a ledger input string, returning the elaboration
    /// result (including errors) rather than panicking.
    fn try_elaborate(input: &str) -> Result<crate::elaboration::Journal, ElaborationError> {
        use crate::{grammars::ledger::parse_ledger, resolution::HIR};
        let ast = parse_ledger(input).expect("parse failed");
        let hir = HIR::try_from(ast).expect("resolution failed");
        crate::elaboration::Journal::try_from(hir)
    }

    #[test]
    fn test_balance_assertion_succeeds_when_balance_matches() {
        let input = "\
2024-01-01 Opening
    Assets:Checking  $1000.00
    Equity:Opening

2024-01-01 = Assets:Checking  $1000.00
";
        // Should elaborate without error.
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_balance_assertion_fails_when_balance_mismatches() {
        let input = "\
2024-01-01 Opening
    Assets:Checking  $1000.00
    Equity:Opening

2024-01-01 = Assets:Checking  $500.00
";
        let result = try_elaborate(input);
        assert!(result.is_err(), "assertion should fail");
        let err = result.unwrap_err();
        match err {
            ElaborationError::BalanceAssertionFailed {
                ref account,
                expected_amount,
                actual_amount,
                ..
            } => {
                assert_eq!(account, "Assets:Checking");
                assert_eq!(expected_amount, dec!(500.00));
                assert_eq!(actual_amount, dec!(1000.00));
            }
            other => panic!("expected BalanceAssertionFailed, got: {other:?}"),
        }
        // Verify Display produces a useful message.
        let msg = err.to_string();
        assert!(
            msg.contains("Assets:Checking"),
            "error should name the account: {msg}"
        );
        assert!(
            msg.contains("500"),
            "error should show expected amount: {msg}"
        );
        assert!(
            msg.contains("1000"),
            "error should show actual amount: {msg}"
        );
    }

    #[test]
    fn test_balance_assertion_zero_balance_at_start() {
        // Asserting zero for an account that has never been posted to should succeed.
        let input = "\
2024-01-01 = Assets:Checking  $0.00
";
        let journal = elaborate(input);
        assert!(journal.transactions.is_empty());
    }

    #[test]
    fn test_balance_assertion_nonzero_at_start_fails() {
        // Asserting a nonzero amount for an account with no postings should fail.
        let input = "\
2024-01-01 = Assets:Checking  $100.00
";
        let result = try_elaborate(input);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ElaborationError::BalanceAssertionFailed { .. }
        ));
    }

    #[test]
    fn test_balance_assertion_after_multiple_transactions() {
        let input = "\
2024-01-01 First deposit
    Assets:Checking  $500.00
    Income:Salary

2024-01-15 Second deposit
    Assets:Checking  $300.00
    Income:Salary

2024-01-31 = Assets:Checking  $800.00
";
        // $500 + $300 = $800 -- assertion should pass.
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);
    }

    #[test]
    fn test_balance_assertion_with_expression() {
        let input = "\
2024-01-01 Opening
    Assets:Checking  $1000.00
    Equity:Opening

2024-01-01 = Assets:Checking  $500.00 + $500.00
";
        // $500 + $500 = $1000 -- assertion should pass.
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_balance_assertion_weak_ignores_other_commodities() {
        // Account holds both $ and EUR. A weak assertion on $ only should pass
        // as long as the $ balance matches -- EUR is ignored.
        let input = "\
2024-01-01 USD deposit
    Assets:Multi  $1000.00
    Equity:Opening

2024-01-02 EUR deposit
    Assets:Multi  500.00 EUR
    Equity:Opening

2024-01-02 = Assets:Multi  $1000.00
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);
    }

    #[test]
    fn test_balance_assertion_between_transactions() {
        // Assertion between two transactions: checks balance at that point.
        let input = "\
2024-01-01 First
    Assets:Checking  $100.00
    Equity:Opening

2024-01-01 = Assets:Checking  $100.00

2024-01-02 Second
    Assets:Checking  $50.00
    Income:Salary

2024-01-02 = Assets:Checking  $150.00
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);
    }

    #[test]
    fn test_balance_assertion_strict_treated_as_weak() {
        // Strict (`==`) currently behaves the same as weak (`=`).
        let input = "\
2024-01-01 Opening
    Assets:Checking  $1000.00
    Equity:Opening

2024-01-01 == Assets:Checking  $1000.00
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    // -- Account-level assert/check tests ------------------------------------─

    /// Helper: attempt elaboration and expect success.
    fn elaborate_ok(input: &str) -> crate::elaboration::Journal {
        elaborate(input)
    }

    /// Helper: attempt elaboration and expect an `AccountAssertionFailed` error.
    fn elaborate_assert_fails(input: &str) -> (String, usize, String) {
        use crate::{grammars::ledger::parse_ledger, resolution::HIR};
        let ast = parse_ledger(input).expect("parse failed");
        let hir = HIR::try_from(ast).expect("resolution failed");
        match crate::elaboration::Journal::try_from(hir).expect_err("expected assertion failure") {
            ElaborationError::AccountAssertionFailed {
                account,
                posting_index,
                expression,
            } => (account, posting_index, expression),
            e => panic!("expected AccountAssertionFailed, got {e:?}"),
        }
    }

    #[test]
    fn test_account_assert_commodity_passes() {
        // Posting to Assets:Checking with "$" commodity -- assertion must pass.
        let input = "\
account Assets:Checking
    assert commodity == \"$\"

2024-01-01 Deposit
    Assets:Checking  $500.00
    Income:Salary
";
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_account_assert_commodity_fails() {
        // Posting with wrong commodity triggers the assertion.
        let input = "\
account Assets:Checking
    assert commodity == \"$\"

2024-01-01 Foreign deposit
    Assets:Checking  500 EUR
    Income:Salary
";
        let (account, posting_index, expression) = elaborate_assert_fails(input);
        assert_eq!(account, "Assets:Checking");
        assert_eq!(posting_index, 0);
        assert!(
            expression.contains("commodity"),
            "expression should mention 'commodity', got: {expression}"
        );
    }

    #[test]
    fn test_account_assert_amount_positive_passes() {
        // Income account asserts amount < 0 (income postings are negative).
        let input = "\
account Income:Salary
    assert amount < 0

2024-01-01 Paycheck
    Income:Salary  $-3000.00
    Assets:Checking
";
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_account_assert_amount_fails() {
        // A positive posting to an income account should trip the assertion.
        let input = "\
account Income:Salary
    assert amount < 0

2024-01-01 Bad entry
    Income:Salary  $100.00
    Assets:Checking
";
        let (account, _, expression) = elaborate_assert_fails(input);
        assert_eq!(account, "Income:Salary");
        assert!(
            expression.contains("amount"),
            "expression should mention 'amount'"
        );
    }

    #[test]
    fn test_account_assert_dimensionless_lhs_compares_with_amount() {
        // `0 < amount` (LHS bare, RHS commodity-bearing) must work the same as
        // `amount > 0`. The commodity-compatibility check on numeric comparisons
        // must be symmetric -- without that, this would error with
        // BinaryOperationTypeError instead of evaluating cleanly.
        let input = "\
account Assets:Savings
    assert 0 < amount

2024-01-01 Deposit
    Assets:Savings  $100.00
    Assets:Checking
";
        // 0 < $100 is true -- assertion passes, elaboration succeeds.
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_account_assert_dimensionless_lhs_fails_when_false() {
        // Same shape as above but the comparison evaluates false -- the
        // assertion should fail (not error with a type mismatch).
        let input = "\
account Assets:Savings
    assert 0 < amount

2024-01-01 Withdrawal
    Assets:Savings  $-50.00
    Assets:Checking
";
        let (account, _, _) = elaborate_assert_fails(input);
        assert_eq!(account, "Assets:Savings");
    }

    #[test]
    fn test_account_assert_no_whitespace_around_cmp_op() {
        // `amount>0` with no spaces around the comparison operator must parse
        // and evaluate correctly. Punctuation operators don't need whitespace
        // (matching arithmetic operators in `value_expr`).
        let input = "\
account Assets:Savings
    assert amount>0

2024-01-01 Deposit
    Assets:Savings  $100.00
    Assets:Checking
";
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_multiple_asserts_all_must_pass() {
        // Two assert lines -- both must hold. First passes, second fails.
        let input = "\
account Assets:Savings
    assert commodity == \"$\"
    assert amount > 0

2024-01-01 Withdrawal
    Assets:Savings  $-100.00
    Assets:Checking
";
        // commodity == "$" passes, but amount > 0 fails for -100.
        let (account, _, _) = elaborate_assert_fails(input);
        assert_eq!(account, "Assets:Savings");
    }

    #[test]
    fn test_account_check_failure_does_not_halt() {
        // `check` produces a warning but elaboration succeeds.
        let input = "\
account Expenses:Food
    check amount > 0

2024-01-01 Refund
    Expenses:Food  $-10.00
    Assets:Checking
";
        // Should NOT error -- check is non-fatal.
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_account_check_passing_no_warning() {
        // check passes silently.
        let input = "\
account Expenses:Food
    check amount > 0

2024-01-01 Dinner
    Expenses:Food  $25.00
    Assets:Checking
";
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    #[test]
    fn test_assert_only_applies_to_declared_account() {
        // Assertion on Assets:Checking does not affect Expenses:Food postings.
        let input = "\
account Assets:Checking
    assert commodity == \"$\"

2024-01-01 Euro lunch
    Expenses:Food  50 EUR
    Assets:Checking  -50 EUR
";
        // The Expenses:Food posting has no assertion -- no error there.
        // The Assets:Checking posting uses EUR, which fails the assertion.
        let (account, _, _) = elaborate_assert_fails(input);
        assert_eq!(account, "Assets:Checking");
    }

    #[test]
    fn test_bool_expr_and_chain_fails_when_rhs_false() {
        // Regression test for issue #78: `and`/`or` in a bool_expr chain were
        // being silently consumed as a commodity by value_expr's postfix, causing
        // the chain to be dropped and the assertion to pass incorrectly.
        //
        // With $50, `amount > 0 and amount < 0` evaluates as `true AND false = false`,
        // so the assertion must fail.
        let input = "\
account Assets:Savings
    assert amount > 0 and amount < 0

2024-01-01 Deposit
    Assets:Savings  $50.00
    Assets:Checking
";
        let (account, _, _) = elaborate_assert_fails(input);
        assert_eq!(account, "Assets:Savings");
    }

    #[test]
    fn test_bool_expr_or_chain_passes_when_either_true() {
        // `amount > 0 or amount < 0` -- true for any nonzero amount.
        // $50 > 0 is true, so the OR chain should pass.
        let input = "\
account Assets:Savings
    assert amount > 0 or amount < 0

2024-01-01 Deposit
    Assets:Savings  $50.00
    Assets:Checking
";
        let journal = elaborate_ok(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tests for balance assignment commodity inference (issue #71)
    // -----------------------------------------------------------------------

    /// A bare `=0` balance assignment should succeed when the account already
    /// has a running balance in exactly one commodity -- the commodity is inferred
    /// from that prior balance, so no explicit commodity or default is needed.
    #[test]
    fn test_balance_assignment_infers_commodity_from_account_balance() {
        // Account A receives $100 in the first transaction, then in the second
        // transaction `=0` brings it back to zero. The posting amount for
        // Account A in the second transaction should be -$100.
        let input = "\
2026-04-01 Setup
    Account A  $100
    Account B

2026-04-02 Zero out
    Account A  =0
    Account B
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);

        let tx = &journal.transactions[1];
        let posting_a = tx
            .postings
            .iter()
            .find(|p| p.account == "Account A")
            .expect("Account A posting not found");

        // The assignment `=0` means: new balance is $0, prior balance is $100,
        // so delta = $0 - $100 = -$100.
        assert_eq!(
            posting_a.amount_in("$"),
            Some(dec!(-100)),
            "balance assignment =0 after $100 should yield -$100 delta"
        );
    }

    /// A balance assignment with an explicit commodity (e.g. `=$0`) should
    /// work exactly as before -- the inferred-commodity path is not taken when
    /// the expression already carries a commodity.
    #[test]
    fn test_balance_assignment_explicit_commodity_still_works() {
        let input = "\
2026-04-01 Setup
    Account A  $100
    Account B

2026-04-02 Zero out
    Account A  =$0
    Account B
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);

        let tx = &journal.transactions[1];
        let posting_a = tx
            .postings
            .iter()
            .find(|p| p.account == "Account A")
            .expect("Account A posting not found");

        assert_eq!(
            posting_a.amount_in("$"),
            Some(dec!(-100)),
            "explicit =$0 should also yield -$100 delta"
        );
    }

    /// A bare `=0` with a `default commodity` directive set should still work
    /// through the existing default-commodity path (no regression).
    #[test]
    fn test_balance_assignment_with_default_commodity() {
        let input = "\
commodity $
    default

2026-04-01 Setup
    Account A  $100
    Account B

2026-04-02 Zero out
    Account A  =0
    Account B
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 2);

        let tx = &journal.transactions[1];
        let posting_a = tx
            .postings
            .iter()
            .find(|p| p.account == "Account A")
            .expect("Account A posting not found");

        assert_eq!(
            posting_a.amount_in("$"),
            Some(dec!(-100)),
            "default-commodity path should yield -$100 delta"
        );
    }

    /// Running balances are not pruned when a commodity reaches zero. A stale
    /// zero-balance entry from an earlier `=$0` should not make a
    /// single-non-zero-commodity account look ambiguous.
    #[test]
    fn test_balance_assignment_ignores_stale_zero_commodities() {
        let input = "\
2026-04-01 Setup USD
    Account A  $100
    Account B

2026-04-02 Zero out USD
    Account A  =$0
    Account B

2026-04-03 Add EUR
    Account A  EUR 50
    Account B

2026-04-04 Zero out (bare)
    Account A  =0
    Account B
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 4);

        // Account A's running balance map at this point is { $: 0, EUR: 50 }.
        // The bare `=0` should infer EUR (the only non-zero commodity), not
        // fail with multi-commodity ambiguity.
        let tx = &journal.transactions[3];
        let posting_a = tx
            .postings
            .iter()
            .find(|p| p.account == "Account A")
            .expect("Account A posting not found");
        assert_eq!(
            posting_a.amount_in("EUR"),
            Some(dec!(-50)),
            "bare =0 should infer the only non-zero commodity (EUR)"
        );
    }

    /// A bare `=0` on an account with no prior balance should still succeed
    /// when the same transaction has another posting establishing the
    /// commodity context. This is the bank-import use case.
    #[test]
    fn test_balance_assignment_infers_commodity_from_same_transaction() {
        // The third posting absorbs the unbalanced amount so the transaction
        // balances; Account B's `=0` itself yields a $0 delta (target $0,
        // prior $0). The key behavior is that Account B is elaborated
        // successfully (no `AmountWithNoCommodity` error) and lands in the
        // expected commodity.
        let input = "\
2026-04-01 Test
    Account A  $100
    Account B  =0
    Account C  $-100
";
        let journal = elaborate(input);
        let tx = &journal.transactions[0];
        let posting_b = tx
            .postings
            .iter()
            .find(|p| p.account == "Account B")
            .expect("Account B posting not found");
        assert!(
            posting_b.amount_in("$").is_some(),
            "bare =0 should infer $ from same-transaction context: {:?}",
            posting_b.amount
        );
        assert_eq!(
            posting_b.amount_in("$"),
            Some(dec!(0)),
            "Account B target is 0 with no prior balance, so delta is 0"
        );
    }

    /// When an account has no prior balance, no transaction context, and no
    /// default commodity, a bare `=0` balance assignment must still error.
    #[test]
    fn test_balance_assignment_no_context_errors() {
        let input = "\
2026-04-01 Test
    Account A  =0
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "bare =0 with no commodity context anywhere should error"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for regex match operators (`=~` / `!~`) -- issue #79
    // -----------------------------------------------------------------------

    /// `assert "abc" =~ /^a/` passes: the string starts with 'a'.
    #[test]
    fn test_regex_match_string_literal_passes() {
        let input = "\
account Expenses:Food
    assert \"abc\" =~ /^a/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `assert "abc" =~ /^z/` fails: the string does not start with 'z'.
    #[test]
    fn test_regex_match_string_literal_fails() {
        let input = "\
account Expenses:Food
    assert \"abc\" =~ /^z/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "regex match should fail when string doesn't match pattern"
        );
    }

    /// `assert "abc" !~ /^z/` passes: the string does not start with 'z'.
    #[test]
    fn test_regex_not_match_passes_when_no_match() {
        let input = "\
account Expenses:Food
    assert \"abc\" !~ /^z/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `assert "abc" !~ /^a/` fails: the string matches, so `!~` is false.
    #[test]
    fn test_regex_not_match_fails_when_match() {
        let input = "\
account Expenses:Food
    assert \"abc\" !~ /^a/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "!~ should fail when string matches pattern"
        );
    }

    /// Regex with a non-trivial pattern including anchors and character classes.
    #[test]
    fn test_regex_match_non_empty_string_pattern() {
        // Pattern `[^\/].+` requires at least two chars and the first isn't a slash.
        let input = "\
account Expenses:Travel
    assert commodity =~ /[a-z]/

2024-01-01 Test
    Expenses:Travel  100 usd
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tests for `tag("name")` function -- issue #80
    // -----------------------------------------------------------------------

    /// Transaction-level metadata is inherited by postings: an assert that
    /// looks up `tag("Entity")` on a posting that has no Entity of its own
    /// should see the transaction-level Entity tag (matches OG ledger-cli).
    #[test]
    fn test_tag_fn_inherits_transaction_metadata() {
        let input = "\
account Expenses:Food
    assert tag(\"Entity\") =~ /^Foo/

2024-01-01 Lunch
    ; Entity: Foo Inc
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// Posting-level metadata wins over transaction-level on key collision.
    #[test]
    fn test_tag_fn_posting_overrides_transaction() {
        let input = "\
account Expenses:Food
    assert tag(\"Entity\") =~ /^Bar/

2024-01-01 Lunch
    ; Entity: Foo Inc
    Expenses:Food  $10.00
    ; Entity: Bar LLC
    Assets:Checking
";
        // Expenses:Food's Entity is Bar LLC (posting wins) -> matches /^Bar/.
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `assert tag("X") =~ /^foo/` passes when posting has `; X: foobar`.
    #[test]
    fn test_tag_fn_matches_metadata_value() {
        let input = "\
account Expenses:Food
    assert tag(\"Entity\") =~ /^foo/

2024-01-01 Test
    Expenses:Food  $10.00
    ; Entity: foobar
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `assert tag("X") !~ /^foo/` passes when posting has no X tag (empty
    /// string does not match `/^foo/`).
    #[test]
    fn test_tag_fn_absent_key_returns_empty_string() {
        let input = "\
account Expenses:Food
    assert tag(\"Entity\") !~ /^foo/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `assert tag("X") =~ /^foo/` fails when posting has no X tag (empty
    /// string does not match a non-empty pattern).
    #[test]
    fn test_tag_fn_absent_key_fails_match() {
        let input = "\
account Expenses:Food
    assert tag(\"Entity\") =~ /^foo/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "tag() on absent key returns empty string, which should not match /^foo/"
        );
    }

    /// Chained check: `tag("A") !~ /^\s*$/ and tag("B") !~ /^\s*$/`.
    /// Both tags present and non-blank -> passes.
    #[test]
    fn test_tag_fn_chained_and_both_present() {
        let input = "\
account Income:Salary
    assert tag(\"Entity\") !~ /^\\s*$/ and tag(\"IncomeType\") !~ /^\\s*$/

2024-01-01 Paycheck
    Income:Salary  $-5000.00
    ; Entity: AcmeCorp
    ; IncomeType: Salary
    Assets:Checking  $5000.00
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// Chained check: one tag present, one absent -> fails.
    #[test]
    fn test_tag_fn_chained_and_one_missing() {
        let input = "\
account Income:Salary
    assert tag(\"Entity\") !~ /^\\s*$/ and tag(\"IncomeType\") !~ /^\\s*$/

2024-01-01 Paycheck
    Income:Salary  $-5000.00
    ; Entity: AcmeCorp
    Assets:Checking  $5000.00
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "chained tag() check should fail when one tag is absent"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for `tag()` with bare (`:foo:`) tags
    // -----------------------------------------------------------------------

    /// `tag()` only inspects key-value metadata (`; Key: value`), not bare
    /// colon-delimited tags (`; :foo:`).  A posting with `:foo:` and an
    /// assertion `tag("foo") =~ /foo/` fails because `tag("foo")` returns ""
    /// (the bare tag name is not in the metadata map).
    ///
    /// This is intentional: bare tags carry no associated value, so `tag()`
    /// returning "" is the correct "not found" signal.  Use bare tags for
    /// filtering via external tooling rather than in `assert`/`check` expressions.
    #[test]
    fn test_tag_fn_does_not_see_bare_colon_tags() {
        let input = "\
account Expenses:Food
    assert tag(\"foo\") =~ /foo/

2024-01-01 Test
    Expenses:Food  $10.00
    ; :foo:
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            result.is_err(),
            "tag() must return \"\" for bare colon-style tags; /foo/ should not match"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for invalid regex error -- issue #79
    // -----------------------------------------------------------------------

    /// An invalid regex pattern in an `assert` expression should fail at
    /// parse time, not silently accepted to fail later during elaboration.
    #[test]
    fn test_invalid_regex_fails_at_parse_time() {
        use crate::grammars::ledger::parse_ledger;
        let input = "\
account Expenses:Food
    assert commodity =~ /[unclosed/
";
        let result = parse_ledger(input);
        let err = result.expect_err("invalid regex should fail parsing");
        let msg = err.to_string();
        assert!(
            msg.contains("[unclosed") && msg.contains("invalid regex"),
            "error message should include the invalid pattern and identify it as a regex; got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for tag directive validation -- issue #82
    // -----------------------------------------------------------------------

    /// `tag X\n    assert value =~ /^foo/` with `; X: foobar` passes.
    #[test]
    fn test_tag_assert_passes_when_value_matches() {
        let input = "\
tag Statement
    assert value =~ /^foo/

2024-01-01 Test
    ; Statement: foobar
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// `tag X\n    assert value =~ /^foo/` with `; X: barfoo` fails with
    /// `TagAssertionFailed`.
    #[test]
    fn test_tag_assert_fails_when_value_does_not_match() {
        let input = "\
tag Statement
    assert value =~ /^foo/

2024-01-01 Test
    ; Statement: barfoo
    Expenses:Food  $10.00
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            matches!(result, Err(ElaborationError::TagAssertionFailed { .. })),
            "expected TagAssertionFailed, got: {result:?}"
        );
        if let Err(ElaborationError::TagAssertionFailed {
            tag_name,
            tag_value,
            ..
        }) = result
        {
            assert_eq!(tag_name, "Statement");
            assert_eq!(tag_value, "barfoo");
        }
    }

    /// `tag X\n    check value =~ /^foo/` with `; X: barfoo` warns but
    /// elaboration succeeds.
    #[test]
    fn test_tag_check_warns_does_not_halt() {
        let input = "\
tag Statement
    check value =~ /^foo/

2024-01-01 Test
    ; Statement: barfoo
    Expenses:Food  $10.00
    Assets:Checking
";
        // Should succeed (check does not halt elaboration).
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// Multiple asserts under one tag: all must pass.
    #[test]
    fn test_tag_multiple_asserts_all_must_pass() {
        let input = "\
tag IncomeType
    assert value =~ /^(Donations|RBI|UBTI)$/

2024-01-01 Income
    ; IncomeType: RBI
    Income:Donations  $100.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// An invalid value fails all asserts.
    #[test]
    fn test_tag_assert_invalid_value_fails() {
        let input = "\
tag IncomeType
    assert value =~ /^(Donations|RBI|UBTI)$/

2024-01-01 Income
    ; IncomeType: Salary
    Income:Salary  $100.00
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            matches!(result, Err(ElaborationError::TagAssertionFailed { .. })),
            "expected TagAssertionFailed for unrecognised IncomeType"
        );
    }

    /// A tag declared but not referenced in any transaction produces no errors.
    #[test]
    fn test_tag_declared_but_unused_no_error() {
        let input = "\
tag Receipt
    assert value =~ /foo/

2024-01-01 Test
    Expenses:Food  $10.00
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// Posting-level metadata is also validated (not just transaction-level).
    #[test]
    fn test_tag_assert_on_posting_level_metadata() {
        let input = "\
tag Statement
    assert value =~ /^foo/

2024-01-01 Test
    Expenses:Food  $10.00
    ; Statement: barfoo
    Assets:Checking
";
        let result = try_elaborate(input);
        assert!(
            matches!(result, Err(ElaborationError::TagAssertionFailed { .. })),
            "posting-level tag metadata should also be validated"
        );
    }

    /// Posting-level metadata with a passing value succeeds.
    #[test]
    fn test_tag_assert_on_posting_level_metadata_passes() {
        let input = "\
tag Statement
    assert value =~ /^foo/

2024-01-01 Test
    Expenses:Food  $10.00
    ; Statement: foobar
    Assets:Checking
";
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    /// Bare colon-tags (e.g. `; :payroll:`) have no value and are NOT validated
    /// by `tag` directive rules -- they are skipped entirely.
    #[test]
    fn test_tag_directive_does_not_validate_bare_colon_tags() {
        let input = "\
tag payroll
    assert value =~ /^.+$/

2024-01-01 Payroll
    ; :payroll:
    Income:Salary  $5000.00
    Assets:Checking
";
        // Bare colon-tags don't have a value, so they never reach the tag
        // directive validator. Elaboration should succeed.
        let journal = elaborate(input);
        assert_eq!(journal.transactions.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tests for parameterized defines (issue #81)
    // -----------------------------------------------------------------------

    /// `define isPositive(x) = x > 0` in an account assert -- passing case.
    #[test]
    fn test_parameterized_define_bool_passing() {
        let input = "\
define isPositive(x) = x > 0

account Expenses:Food
    assert isPositive(amount)

2024-01-01 Lunch
    Expenses:Food  $10.00
    Assets:Cash
";
        elaborate(input);
    }

    /// `define isNegative(x) = x < 0` in an account assert -- positive amount
    /// should trigger the assertion.
    #[test]
    fn test_parameterized_define_bool_failing() {
        let input = "\
define isNegative(x) = x < 0

account Expenses:Food
    assert isNegative(amount)

2024-01-01 Lunch
    Expenses:Food  $10.00
    Assets:Cash
";
        let ast = crate::grammars::ledger::parse_ledger(input).expect("parse failed");
        let hir = crate::resolution::HIR::try_from(ast).expect("resolution failed");
        let result = crate::elaboration::Journal::try_from(hir);
        assert!(
            matches!(result, Err(ElaborationError::AccountAssertionFailed { .. })),
            "positive amount should fail isNegative assertion; got: {result:?}"
        );
    }

    /// Bool define using `tag()` and `!~` regex match.
    #[test]
    fn test_parameterized_define_with_tag_and_regex_passing() {
        let input = "\
define hasReceipt(x) = tag(\"Receipt\") !~ /^\\s*$/ and x > 0

account Expenses:Food
    assert hasReceipt(amount)

2024-01-01 Lunch
    Expenses:Food  $10.00
    ; Receipt: scan123.pdf
    Assets:Cash
";
        elaborate(input);
    }

    /// Two-argument define: `between(lo, hi) = amount > lo and amount < hi`
    /// where `amount` is in scope from the posting context.
    #[test]
    fn test_parameterized_define_two_args_passing() {
        let input = "\
define between(lo, hi) = amount > lo and amount < hi

account Expenses:Food
    assert between(0, 100)

2024-01-01 Lunch
    Expenses:Food  $50.00
    Assets:Cash
";
        elaborate(input);
    }

    /// Same `between` define -- amount outside range fails.
    #[test]
    fn test_parameterized_define_two_args_failing() {
        let input = "\
define between(lo, hi) = amount > lo and amount < hi

account Expenses:Food
    assert between(0, 10)

2024-01-01 BigPurchase
    Expenses:Food  $50.00
    Assets:Cash
";
        let ast = crate::grammars::ledger::parse_ledger(input).expect("parse failed");
        let hir = crate::resolution::HIR::try_from(ast).expect("resolution failed");
        let result = crate::elaboration::Journal::try_from(hir);
        assert!(
            matches!(result, Err(ElaborationError::AccountAssertionFailed { .. })),
            "$50 should fail between(0, 10); got: {result:?}"
        );
    }

    /// Param named `amount` shadows the implicit posting binding.
    #[test]
    fn test_parameterized_define_param_shadows_amount() {
        let input = "\
define isPositiveAmt(amount) = amount > 0

account Expenses:Food
    assert isPositiveAmt(amount)

2024-01-01 Lunch
    Expenses:Food  $10.00
    Assets:Cash
";
        elaborate(input);
    }

    /// A zero-parameter value-body define continues to work as before.
    #[test]
    fn test_zero_param_define_value_body_still_works() {
        let input = "\
define monthly = $1500.00

2024-01-01 Rent
    Expenses:Rent  monthly
    Assets:Cash
";
        let journal = elaborate(input);
        let rent = journal.transactions[0]
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Rent")
            .unwrap();
        assert_eq!(rent.amount_in("$"), Some(dec!(1500.00)));
    }

    /// Mutually-recursive defines must produce a `RecursionLimitExceeded`
    /// error rather than crash the process with a stack overflow.
    #[test]
    fn test_mutually_recursive_defines_caught() {
        let input = "\
define a = b
define b = a

2024-01-01 Test
    Expenses:Food  a
    Assets:Cash
";
        let result = try_elaborate(input);
        assert!(
            matches!(
                result,
                Err(ElaborationError::EvaluationError(
                    EvaluationError::RecursionLimitExceeded
                ))
            ),
            "cyclic defines should produce RecursionLimitExceeded; got: {result:?}"
        );
    }

    /// Self-referential define caught the same way.
    #[test]
    fn test_self_referential_define_caught() {
        let input = "\
define x = x

2024-01-01 Test
    Expenses:Food  x
    Assets:Cash
";
        let result = try_elaborate(input);
        assert!(
            matches!(
                result,
                Err(ElaborationError::EvaluationError(
                    EvaluationError::RecursionLimitExceeded
                ))
            ),
            "self-referential define should produce RecursionLimitExceeded; got: {result:?}"
        );
    }

    /// A parameterized value-body define can be used in a posting amount.
    #[test]
    fn test_parameterized_define_value_body_in_posting() {
        let input = "\
define double(x) = x * 2

2024-01-01 Purchase
    Expenses:Food  double(50 USD)
    Assets:Cash
";
        let journal = elaborate(input);
        let food = journal.transactions[0]
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(food.amount_in("USD"), Some(dec!(100)));
    }

    // -- Issue #89: parenthesised bool expressions in value/bool positions --

    #[test]
    fn test_paren_bool_simple_assert_passes() {
        // `(amount > 0)` in an account assert should pass for a positive posting.
        let input = "\
account Assets:Savings
    assert (amount > 0)

2024-01-01 Deposit
    Assets:Savings  $100.00
    Assets:Cash
";
        elaborate(input); // must not panic
    }

    #[test]
    fn test_paren_bool_or_chain_passes_when_first_true() {
        // `(amount > 0 or amount < -10)` -- first arm true, should pass.
        let input = "\
account Assets:Savings
    assert (amount > 0 or amount < -10)

2024-01-01 Deposit
    Assets:Savings  $100.00
    Assets:Cash
";
        elaborate(input);
    }

    #[test]
    fn test_paren_bool_or_chain_passes_when_second_true() {
        // `(amount > 0 or amount < -10)` -- second arm true.
        let input = "\
account Assets:Savings
    assert (amount > 0 or amount < -10)

2024-01-01 Withdrawal
    Assets:Cash  $100.00
    Assets:Savings  $-100.00
";
        // $-100 satisfies `amount < -10` (the second branch).
        // NOTE: amount here would be -100 which is < -10 -> passes.
        elaborate(input);
    }

    #[test]
    fn test_define_paren_bool_used_in_assert() {
        // A parameterized define whose body is a parenthesised bool_expr,
        // then used in an account assert.
        let input = "\
define inRange(x) = (x > 0 and x < 1000)

account Assets:Savings
    assert inRange(amount)

2024-01-01 Deposit
    Assets:Savings  $100.00
    Assets:Cash
";
        elaborate(input);
    }

    #[test]
    fn test_issue_89_define_with_complex_paren_bool() {
        // The exact pattern from issue #89 (simplified to avoid needing real
        // metadata -- just verify it elaborates without error when the outer
        // `or` short-circuits on the amount comparison).
        let input = "\
define assetChecker(amt) = (amt > -100.00 or (tag(\"TaxImplication\") !~ /^\\s*$/ and tag(\"Entity\") !~ /^\\s*$/))

account Assets:Savings
    assert assetChecker(amount)

2024-01-01 Deposit
    Assets:Savings  $500.00
    Assets:Cash
";
        // amount=500 > -100 -> outer `or` short-circuits to true.
        elaborate(input);
    }

    // --------------------------------------------------------------------------
    // Virtual posting unit tests (#140)
    // --------------------------------------------------------------------------

    /// A transaction with a real posting, a virtual-unbalanced posting, and
    /// a null posting: the unbalanced posting must not affect the null-posting
    /// inference (i.e. the null posting absorbs only the real posting's amount).
    #[test]
    fn virtual_unbalanced_does_not_affect_null_posting_inference() {
        let input = "\
2024-01-15 Test
    Assets:Checking           $100
    (Equity:Reservations)     $-25
    Equity:Opening
";
        let j = elaborate(input);
        let t = &j.transactions[0];
        assert_eq!(t.postings.len(), 3);

        // Null posting should be inferred as -$100 (negation of real $100),
        // not -$75 (which would incorrectly include the virtual unbalanced).
        let null_p = t
            .postings
            .iter()
            .find(|p| p.account == "Equity:Opening")
            .expect("null posting present");
        assert_eq!(null_p.amount_in("$"), Some(dec!(-100)));

        let virt = t
            .postings
            .iter()
            .find(|p| p.account == "Equity:Reservations")
            .expect("virtual posting present");
        assert_eq!(virt.amount_in("$"), Some(dec!(-25)));

        use crate::elaboration::PostingKind;
        assert_eq!(virt.kind, PostingKind::VirtualUnbalanced as i32);
        assert_eq!(null_p.kind, PostingKind::Real as i32);
    }

    /// A transaction with a real posting, a virtual-balanced posting, and a
    /// null posting: the balanced posting participates in the null-posting
    /// inference (the null absorbs the sum of real + balanced).
    #[test]
    fn virtual_balanced_participates_in_null_posting_inference() {
        let input = "\
2024-01-15 Test
    Assets:Checking           $100
    [Equity:Reservations]     $25
    Equity:Opening
";
        let j = elaborate(input);
        let t = &j.transactions[0];
        assert_eq!(t.postings.len(), 3);

        // Null posting absorbs -(100 + 25) = -$125 because the balanced
        // virtual posting contributes to the transaction state.
        let null_p = t
            .postings
            .iter()
            .find(|p| p.account == "Equity:Opening")
            .expect("null posting present");
        assert_eq!(null_p.amount_in("$"), Some(dec!(-125)));

        let virt = t
            .postings
            .iter()
            .find(|p| p.account == "Equity:Reservations")
            .expect("virtual posting present");
        assert_eq!(virt.amount_in("$"), Some(dec!(25)));

        use crate::elaboration::PostingKind;
        assert_eq!(virt.kind, PostingKind::VirtualBalanced as i32);
    }

    /// A transaction consisting solely of virtual-unbalanced postings should
    /// elaborate without a balance error: there are no real postings to balance.
    #[test]
    fn transaction_with_only_virtual_unbalanced_postings_does_not_error() {
        let input = "\
2024-01-15 Memo-only entry
    (Budget:Food)    $50
    (Budget:Travel)  $-50
";
        let j = elaborate(input);
        let t = &j.transactions[0];
        assert_eq!(t.postings.len(), 2);

        use crate::elaboration::PostingKind;
        for p in &t.postings {
            assert_eq!(p.kind, PostingKind::VirtualUnbalanced as i32);
        }
    }

    /// A virtual-unbalanced posting must update the running per-account balance
    /// so that a subsequent standalone balance assertion on the same account
    /// reflects the virtual amount -- matching ledger-cli behaviour.
    #[test]
    fn virtual_unbalanced_posting_updates_account_balance_for_assertions() {
        // The virtual posting credits $-25 to Equity:Reservations.
        // A subsequent balance assertion checks that the account balance is $-25,
        // which should succeed because virtual-unbalanced postings contribute to
        // account_balances even though they're excluded from the transaction check.
        let input = "\
2024-01-15 Setup
    Assets:Checking           $100
    (Equity:Reservations)     $-25
    Equity:Opening

2024-01-15 = Equity:Reservations  $-25
";
        // Should elaborate without error -- the balance assertion sees the virtual
        // posting's contribution.
        let j = elaborate(input);
        assert_eq!(j.transactions.len(), 1);

        let virt = j.transactions[0]
            .postings
            .iter()
            .find(|p| p.account == "Equity:Reservations")
            .expect("virtual posting present");
        assert_eq!(virt.amount_in("$"), Some(dec!(-25)));
    }

    // ----------------------------------------------------------------------
    // Lot annotation elaborator tests
    // ----------------------------------------------------------------------

    #[test]
    fn test_lot_cost_only_drives_cash_balance() {
        // 10 AAPL {$150} (no @ price) -> cash side -$1500.
        let input = "\
2024-03-01 Buy AAPL
    Assets:Brokerage   10 AAPL {$150}
    Assets:Cash
";
        let journal = elaborate(input);
        let t = &journal.transactions[0];
        assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
        // No @/@@, cost annotation drives the cash balance: 10 * $150 = $1500.
        assert_eq!(
            t.postings[1].amount_in("$"),
            Some(dec!(-1500)),
            "cash side should be -$1500 when lot cost drives balance"
        );
        // Lot cost preserved on the proto posting.
        assert_eq!(t.postings[0].lot_cost_in("$"), Some(dec!(150)));
    }

    #[test]
    fn test_lot_cost_and_price_price_wins_cash_cost_preserved() {
        // 10 AAPL {$150} @ $155 -> cash -$1550 (price drives balance),
        // but lot.cost = $150 is still stored.
        let input = "\
2024-03-01 Buy AAPL
    Assets:Brokerage   10 AAPL {$150} @ $155
    Assets:Cash
";
        let journal = elaborate(input);
        let t = &journal.transactions[0];
        assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
        // Price wins over lot cost for cash balance: 10 * $155 = $1550.
        assert_eq!(
            t.postings[1].amount_in("$"),
            Some(dec!(-1550)),
            "cash side should be -$1550 when @ price is present"
        );
        // Lot cost annotation is preserved even though it didn't drive balance.
        assert_eq!(
            t.postings[0].lot_cost_in("$"),
            Some(dec!(150)),
            "lot cost should be $150, not the price $155"
        );
    }

    #[test]
    fn test_lot_no_cost_no_price_value_in_own_commodity() {
        // 10 AAPL (no annotation, no price) -> the null posting balances in
        // AAPL (today's fallback: the commodity contributes itself).
        let input = "\
2024-03-01 Transfer
    Assets:Brokerage   10 AAPL
    Assets:OtherBrokerage
";
        let journal = elaborate(input);
        let t = &journal.transactions[0];
        assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
        // Null posting inferred as -10 AAPL.
        assert_eq!(
            t.postings[1].amount_in("AAPL"),
            Some(dec!(-10)),
            "null posting should balance as -10 AAPL when no price is given"
        );
        assert!(!t.postings[0].has_lot(), "no lot annotation should be set");
    }
}
