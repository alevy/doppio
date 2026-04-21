//! Elaboration stage: evaluate expressions, balance transactions, and
//! produce the final serialisable [`Journal`].
//!
//! This stage converts a [`resolution::HIR`] into an [`elaboration::Journal`]
//! by performing the following work:
//!
//! - **Expression evaluation** — [`ast::ValueExpr`] trees are evaluated to
//!   concrete `(Decimal, commodity)` pairs by the [`evaluator`] submodule.
//!   Commodity aliases from the active [`resolution::Context`] are applied.
//!
//! - **Transaction balancing** — if a transaction has exactly one posting with
//!   no explicit amount (a "null posting"), its amount is inferred as the
//!   negation of all other postings' sum. If all postings have amounts their
//!   sum must be zero; otherwise [`ElaborationError::TransactionDoesNotBalance`]
//!   is returned.
//!
//! - **Balance assertions / assignments** — `= expected` checks are verified
//!   against the running account balance. `= target` assignments set the
//!   posting amount to `target − current_balance`.
//!
//! - **Lot pricing** — `@ unit` and `@@ total` cost annotations are converted
//!   into a cash amount in the lot's commodity for the purpose of balancing.
//!
//! - **Account registration** — every account mentioned in a posting is added
//!   to [`Journal::accounts`], merging any properties declared in `account`
//!   directives.

use std::{collections::BTreeMap, fmt::Display};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    ast::{self, AmountDetails, ValueExpr},
    resolution,
};

/// The fully elaborated journal: the final output of the compilation pipeline.
///
/// `Journal` implements [`serde::Serialize`] and [`serde::Deserialize`] so it
/// can be written to a `.bki` file (via `postcard` + XZ) and read back later
/// without re-parsing the source.
#[derive(Debug, Serialize, Deserialize)]
pub struct Journal {
    /// All transactions in source order, with amounts fully evaluated.
    pub transactions: Vec<ResolvedTransaction>,
    /// All accounts referenced by any posting, with their declared properties.
    pub accounts: BTreeMap<String, AccountProperties>,
    /// Market price quotes from `P` directives, in source order, with the
    /// price expression fully evaluated to a concrete `(Decimal, commodity)`.
    pub prices: Vec<HistoricalPrice>,
}

/// A fully evaluated historical price entry produced from a `P` directive.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoricalPrice {
    /// Days since the Unix epoch on which this price was recorded.
    pub date: i32,
    /// Optional wall-clock time of the price quote (`"HH:MM"` or `"HH:MM:SS"`).
    pub time: Option<String>,
    /// The commodity whose price is being recorded (e.g. `"AAPL"`, `"BTC"`).
    pub commodity: String,
    /// The evaluated price of one unit of `commodity`.
    pub price: Decimal,
    /// The commodity the price is expressed in (e.g. `"$"`, `"USD"`).
    pub price_commodity: Commodity,
}

/// Properties of an account declared with an `account` directive.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AccountProperties {
    /// A free-form note describing the account.
    pub note: Option<String>,
}

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

/// A fully evaluated and balanced transaction, ready for serialisation.
#[derive(Debug, Deserialize, Serialize)]
pub struct ResolvedTransaction {
    /// Days since the Unix epoch (1970-01-01 = 0).
    ///
    /// Stored as `i32` rather than a `NaiveDate` because `i32` serialises
    /// compactly with postcard (4 bytes, fixed-width), is trivially sortable,
    /// and avoids embedding chrono's internal representation in the on-disk
    /// format.
    pub date: i32,
    /// Optional secondary date in the same epoch-days format.
    pub secondary_date: Option<i32>,
    /// Cleared / pending / uncleared state.
    pub state: TransactionState,
    /// Optional reference code from the transaction header.
    pub code: Option<String>,
    /// The payee / description.
    pub description: String,
    /// Tags extracted from header notes.
    pub tags: Vec<String>,
    /// Key-value metadata extracted from header notes.
    pub metadata: BTreeMap<String, String>,
    /// The resolved postings (all amounts concrete, null posting filled in).
    pub postings: Vec<ResolvedPosting>,
}

/// A posting with a fully evaluated, concrete amount.
#[derive(Deserialize, Serialize, Debug)]
pub struct ResolvedPosting {
    /// The canonical account name (after alias resolution).
    pub account: String,
    /// The payee for this posting — taken from posting-level `payee:` metadata
    /// if present, otherwise inherited from the transaction description.
    pub payee: String,
    /// The posting amount, keyed by commodity.
    pub amount: Amount,
    /// Per-posting state.
    pub state: TransactionState,
    /// Tags extracted from posting notes.
    pub tags: Vec<String>,
    /// Key-value metadata from posting notes.
    pub metadata: BTreeMap<String, String>,
}

/// A commodity name (e.g. `"USD"`, `"BTC"`, `"$"`).
pub type Commodity = String;

/// A multi-commodity amount: a map from commodity symbol to a `Decimal` value.
///
/// The serde representation uses `[u8; 16]` per value — the fixed-size binary
/// encoding produced by [`rust_decimal::Decimal::serialize`] — so the `.bki`
/// wire format is identical to the original byte-array storage. The conversion
/// is handled by the manual `Serialize`/`Deserialize` impls below.
#[derive(Default, Debug)]
pub struct Amount(pub BTreeMap<Commodity, Decimal>);

impl serde::Serialize for Amount {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let bytes_map: BTreeMap<&Commodity, [u8; 16]> =
            self.0.iter().map(|(k, v)| (k, v.serialize())).collect();
        bytes_map.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for Amount {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes_map = BTreeMap::<Commodity, [u8; 16]>::deserialize(d)?;
        Ok(Amount(
            bytes_map
                .into_iter()
                .map(|(k, v)| (k, Decimal::deserialize(v)))
                .collect(),
        ))
    }
}

/// Cleared/pending state of a resolved transaction or posting.
///
/// This is a separate type from [`ast::TransactionState`] because it derives
/// `Serialize`/`Deserialize` for the on-disk format; `ast::TransactionState`
/// does not need those traits.
#[derive(Deserialize, Serialize, Debug)]
pub enum TransactionState {
    /// No state marker.
    Uncleared,
    /// `!` — pending confirmation.
    Pending,
    /// `*` — confirmed / reconciled.
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
    /// A `= expected` balance assertion failed: the account's balance after
    /// this posting does not equal the asserted value.
    PostingBalanceAssertionFailed,
    /// A transaction has more than one null posting (only one is allowed, since
    /// multiple unknowns cannot be uniquely determined).
    TooManyNullPostings,
    /// All postings have explicit amounts but they do not sum to zero.
    TransactionDoesNotBalance(Amount),
}

/// Errors from evaluating a [`ast::ValueExpr`].
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
}

impl From<EvaluationError> for ElaborationError {
    fn from(e: EvaluationError) -> ElaborationError {
        ElaborationError::EvaluationError(e)
    }
}

impl std::error::Error for ElaborationError {}

impl Display for ElaborationError {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl TryFrom<resolution::HIR> for Journal {
    type Error = ElaborationError;

    fn try_from(value: resolution::HIR) -> Result<Self, Self::Error> {
        let mut state = RunningState::default();

        let mut transactions = vec![];

        // Pre-populate the accounts map from directives so that accounts
        // declared with notes but never posted to still appear in the output.
        let mut accounts = BTreeMap::new();
        for (name, properties) in value.global_context.account_properties {
            accounts.insert(
                name,
                AccountProperties {
                    note: properties.note,
                },
            );
        }

        for entry in value.entries {
            let entry_context = &value.contexts[entry.context_id];
            match entry.data {
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
                        if let Some(amount) = posting.amount {
                            let account_name = entry_context
                                .account_aliases
                                .get(&posting.account)
                                .cloned()
                                .unwrap_or(posting.account);
                            let account_balance = state.account_balances.get(&account_name);
                            let (value, commodity, lot_pricing) = match amount {
                                AmountDetails::Amount {
                                    value,
                                    lot_pricing,
                                    balance_assertion,
                                } => {
                                    let (value, commodity) = evaluator::eval_and_normalize_amount(
                                        value,
                                        entry_context,
                                        &state,
                                    )?;
                                    let lot_pricing = match lot_pricing {
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
                                            // "@ unit_price" — total cash = units * price
                                            let (v, c) = evaluator::eval_and_normalize_amount(
                                                expr,
                                                entry_context,
                                                &state,
                                            )?;
                                            Some((v * value, c))
                                        }
                                        None => None,
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
                                        // *before* this posting — consistent with ledger-cli.
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
                                    (value, commodity, lot_pricing)
                                }
                                AmountDetails::BalanceAssignment(assignment) => {
                                    // "= target_balance" — compute the delta needed to reach the
                                    // target from the current running balance.
                                    let (newsum, commodity) = evaluator::eval_and_normalize_amount(
                                        assignment,
                                        entry_context,
                                        &state,
                                    )?;
                                    let value = newsum
                                        - account_balance
                                            .and_then(|ab| ab.commodity.get(&commodity))
                                            .unwrap_or(&Decimal::ZERO);
                                    (value, commodity, None)
                                }
                            };
                            let payee = posting.metadata.remove("payee").unwrap_or(payee.clone());

                            // For lot-priced postings, add the *cash* total (in the lot's
                            // commodity) to transaction_state rather than the commodity units.
                            // This is what needs to balance with the offsetting cash posting.
                            if let Some((lot_total, lot_commodity)) = lot_pricing {
                                let dec = transaction_state.0.entry(lot_commodity).or_default();
                                *dec = *dec + lot_total;
                            } else {
                                let dec =
                                    transaction_state.0.entry(commodity.clone()).or_default();
                                *dec = *dec + value;
                            }

                            let amount = Amount(BTreeMap::from([(commodity, value)]));
                            resolved_postings.push(ResolvedPosting {
                                account: account_name,
                                payee,
                                amount,
                                state: posting.state.into(),
                                tags: posting.tags,
                                metadata: posting.metadata,
                            });
                        } else {
                            // Defer processing and save for next step
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
                        // other postings, making the transaction balance to zero.
                        let amount = Amount(
                            transaction_state
                                .0
                                .iter()
                                .map(|(c, v)| (c.clone(), -v))
                                .collect(),
                        );

                        resolved_postings.push(ResolvedPosting {
                            account: account_name,
                            payee,
                            amount,
                            state: posting.state.into(),
                            tags: posting.tags,
                            metadata: posting.metadata,
                        });
                    } else {
                        // Check that transaction state is all zeros to balance the transaction.
                        if transaction_state
                            .0
                            .values()
                            .any(|value| !value.is_zero())
                        {
                            return Err(ElaborationError::TransactionDoesNotBalance(
                                transaction_state,
                            ));
                        }
                    }

                    // Update running account balances and register new accounts.
                    for posting in resolved_postings.iter() {
                        if !accounts.contains_key(&posting.account) {
                            accounts.insert(posting.account.clone(), Default::default());
                        }

                        let balances = state
                            .account_balances
                            .entry(posting.account.clone())
                            .or_default();
                        for (commodity, delta) in posting.amount.0.iter() {
                            *(balances.commodity.entry(commodity.clone()).or_default()) +=
                                delta;
                        }
                    }

                    transactions.push(ResolvedTransaction {
                        date: transaction.date.to_epoch_days(),
                        secondary_date: transaction.secondary_date.map(|d| d.to_epoch_days()),
                        state: transaction.state.into(),
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
        let final_context = value.contexts.last().expect("HIR always has at least one context");
        let mut prices = vec![];
        for hp in value.prices {
            let (price, price_commodity) =
                evaluator::eval_and_normalize_amount(hp.price, final_context, &state)?;
            prices.push(HistoricalPrice {
                date: hp.date.to_epoch_days(),
                time: hp.time,
                commodity: hp.commodity,
                price,
                price_commodity,
            });
        }

        Ok(Journal {
            transactions,
            accounts,
            prices,
        })
    }
}

/// Expression evaluator: reduces [`ast::ValueExpr`] trees to concrete values.
mod evaluator {
    use std::collections::BTreeMap;

    use rust_decimal::Decimal;

    use crate::{
        ast::{self, ValueExpr},
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
        match eval(val, eval_context, running_state)? {
            ast::ValueExpr::Amount { value, commodity } => {
                let commodity = if let Some(commodity) = commodity {
                    // Apply commodity alias (e.g. "Bitcoin" → "BTC")
                    eval_context
                        .commodity_aliases
                        .get(&commodity)
                        .unwrap_or(&commodity)
                        .clone()
                } else {
                    // No commodity in the expression — use the context default
                    eval_context
                        .default_commodity
                        .clone()
                        .ok_or(ElaborationError::AmountWithNoCommodity)?
                };
                Ok((value, commodity))
            }
            val => Err(ElaborationError::NonAmountWhereAmountExpected(val)),
        }
    }

    /// Recursively evaluate a [`ast::ValueExpr`] to a simpler form.
    ///
    /// The evaluator reduces arithmetic, applies unary operators, resolves
    /// function calls, and handles type annotations. It does not resolve
    /// commodity aliases — that is done by `eval_and_normalize_amount`.
    fn eval(
        val: ast::ValueExpr,
        eval_context: &resolution::Context,
        state: &RunningState,
    ) -> Result<ast::ValueExpr, EvaluationError> {
        match val {
            // Base cases: already-reduced values pass through unchanged.
            a @ ast::ValueExpr::Amount { .. } => Ok(a),
            s @ ast::ValueExpr::Str(_) => Ok(s),
            o @ ast::ValueExpr::Object(_) => Ok(o),

            ast::ValueExpr::Unary { op, expr } => match eval(*expr, eval_context, state)? {
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
            },

            ast::ValueExpr::Binary { lhs, rhs, op } => {
                match (
                    eval(*lhs, eval_context, state)?,
                    eval(*rhs, eval_context, state)?,
                ) {
                    // One side has a commodity, the other is dimensionless —
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

                    // Both sides have the same commodity — straightforward arithmetic.
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

            ast::ValueExpr::Function { name, args } => match (name.as_str(), args.as_slice()) {
                // scrub(x) — identity function used by some ledger-cli extensions
                // to mark amounts as "scrubbed" (processed). Treated as a no-op.
                ("scrub", [arg]) => eval(arg.clone(), eval_context, state),

                // account("Name") — returns an object with a "total" field
                // containing the current running balance of the named account.
                // Only the primary commodity ($) is currently surfaced.
                ("account", [account]) => {
                    if let ValueExpr::Str(account) = eval(account.clone(), eval_context, state)? {
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
                _ => Err(EvaluationError::UnknownFunctionArgs((name, args))),
            },

            // A bare commodity symbol — returned as-is; the Binary handler
            // above resolves it when combined with an adjacent number.
            c @ ast::ValueExpr::Commodity(_) => Ok(c),

            ast::ValueExpr::Typed {
                expr,
                commodity: new_commodity,
            } => match eval(*expr, eval_context, state)? {
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

            ast::ValueExpr::Access { expr, field } => match eval(*expr, eval_context, state)? {
                ast::ValueExpr::Object(map) => map
                    .get(&field)
                    .cloned()
                    .ok_or(EvaluationError::NoSuchField(field)),
                val => Err(EvaluationError::FieldAccessTypeError(val)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn test_amount_serde_wire_format() {
        // Verify that Amount serializes identically to BTreeMap<Commodity, [u8; 16]>,
        // preserving the binary wire format for .bki files.
        let decimal = dec!(182.50);
        let amount = Amount(BTreeMap::from([("$".to_string(), decimal)]));

        // Serialize via our custom impl
        let amount_bytes = postcard::to_allocvec(&amount).unwrap();

        // Serialize the equivalent [u8; 16] map directly
        let raw_map: BTreeMap<&str, [u8; 16]> = BTreeMap::from([("$", decimal.serialize())]);
        let raw_bytes = postcard::to_allocvec(&raw_map).unwrap();

        assert_eq!(amount_bytes, raw_bytes, "Amount wire format must match [u8;16] map");
    }

    #[test]
    fn test_amount_serde_roundtrip() {
        let decimal = dec!(42.123456789);
        let original = Amount(BTreeMap::from([
            ("USD".to_string(), decimal),
            ("$".to_string(), dec!(-1.5)),
        ]));
        let bytes = postcard::to_allocvec(&original).unwrap();
        let recovered: Amount = postcard::from_bytes(&bytes).unwrap();

        assert_eq!(original.0.len(), recovered.0.len());
        for (k, v) in &original.0 {
            assert_eq!(recovered.0[k], *v);
        }
    }

    #[test]
    fn test_prices_wired_through_to_journal() {
        use crate::{ast, resolution};

        // Build an AST journal with a single P directive.
        let price_ast = ast::HistoricalPrice {
            date: ast::Date { year: Some(2024), month: 6, date: 15 },
            time: Some("14:30:00".into()),
            commodity: "AAPL".into(),
            price: ast::ValueExpr::amount(
                rust_decimal::Decimal::from(182),
                "$".into(),
            ),
        };
        let journal_ast = ast::Journal {
            entries: vec![ast::Entry::HistoricalPrice(price_ast)],
        };

        // Resolution stage.
        let hir = resolution::HIR::try_from(journal_ast)
            .expect("resolution should succeed");
        assert_eq!(hir.prices.len(), 1, "HIR should contain one price");

        // Elaboration stage.
        let journal = Journal::try_from(hir)
            .expect("elaboration should succeed");

        assert_eq!(journal.prices.len(), 1, "Journal should contain one price");
        let price = &journal.prices[0];

        // date: 2024-06-15 → days since epoch
        let expected_days = chrono::NaiveDate::from_ymd_opt(2024, 6, 15)
            .unwrap()
            .to_epoch_days();
        assert_eq!(price.date, expected_days);
        assert_eq!(price.time.as_deref(), Some("14:30:00"));
        assert_eq!(price.commodity, "AAPL");
        assert_eq!(price.price, rust_decimal::Decimal::from(182));
        assert_eq!(price.price_commodity, "$");
    }
}
