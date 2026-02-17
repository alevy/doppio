use std::{collections::BTreeMap, fmt::Display};

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    ast::{self, AmountDetails, ValueExpr},
    resolution,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Journal {
    pub transactions: Vec<ResolvedTransaction>,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct AccountBalances {
    #[serde(flatten)]
    pub commodity: BTreeMap<String, Decimal>,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct RunningState {
    #[serde(flatten)]
    pub account_balances: BTreeMap<String, AccountBalances>,
}
impl RunningState {
    fn minify(mut self) -> RunningState {
        for (_, ab) in self.account_balances.iter_mut() {
            ab.commodity.retain(|_, v| !v.is_zero());
        }

        self.account_balances
            .retain(|_, ab| !ab.commodity.is_empty());
        self
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ResolvedTransaction {
    pub date: NaiveDate,
    pub secondary_date: Option<NaiveDate>,
    pub state: TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub running_state: RunningState,
    pub postings: Vec<ResolvedPosting>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ResolvedPosting {
    pub account: String,
    pub payee: String,
    pub amount: Amount,
    pub state: TransactionState,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

pub type Commodity = String;

#[derive(Deserialize, Serialize, Debug)]
pub struct Amount(BTreeMap<Commodity, Decimal>);

#[derive(Deserialize, Serialize, Debug)]
pub enum TransactionState {
    Uncleared,
    Pending,
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

#[derive(Debug)]
pub enum ElaborationError {
    AmountWithNoCommodity,
    NonAmountWhereAmountExpected(ValueExpr),
    EvaluationError(EvaluationError),
    PostingBalanceAssertionFailed,
    TooManyNullPostings,
    TransactionDoesNotBalance(Amount),
}

#[derive(Debug)]
pub enum EvaluationError {
    UnaryMultiplyOrDivide,
    UnaryOnNonAmount(ValueExpr),
    BinaryOperationTypeError((ValueExpr, ValueExpr, crate::ast::Op)),
    NoSuchField(String),
    FieldAccessTypeError(ValueExpr),
    UnknownFunctionArgs((String, Vec<ValueExpr>)),
    TypedCommodityToIncompatibleAmount((String, ValueExpr)),
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

        for entry in value.entries {
            let entry_context = &value.contexts[entry.context_id];
            match entry.data {
                resolution::Entry::Transaction(mut transaction) => {
                    let mut transaction_state = Amount(BTreeMap::default());

                    let payee = transaction
                        .metadata
                        .remove("payee")
                        .unwrap_or_else(|| transaction.description.clone());
                    let mut null_postings = vec![];
                    let mut resolved_postings = vec![];
                    for mut posting in transaction.postings {
                        if let Some(amount) = posting.amount {
                            let account_name = entry_context
                                .account_aliases
                                .get(&posting.account)
                                .cloned()
                                .unwrap_or(posting.account);
                            let account_balance = state
                                .account_balances
                                .entry(account_name.clone())
                                .or_default();
                            let (value, commodity, lot_pricing) = match amount {
                                AmountDetails::Amount {
                                    value,
                                    lot_pricing,
                                    balance_assertion,
                                } => {
                                    let (value, commodity) = evaluator::eval_and_normalize_amount(
                                        value,
                                        &entry_context,
                                    )?;
                                    let lot_pricing = match lot_pricing {
                                        Some(ast::LotPricing::Total(expr)) => {
                                            let (mut v, c) = evaluator::eval_and_normalize_amount(
                                                expr,
                                                &entry_context,
                                            )?;
                                            if value.is_sign_negative() {
                                                v = -v;
                                            }
                                            Some((v, c))
                                        }
                                        Some(ast::LotPricing::Unit(expr)) => {
                                            let (v, c) = evaluator::eval_and_normalize_amount(
                                                expr,
                                                &entry_context,
                                            )?;
                                            Some((v * value, c))
                                        }
                                        None => None,
                                    };
                                    if let Some(balance_assertion) = balance_assertion {
                                        let (baval, bacommodity) =
                                            evaluator::eval_and_normalize_amount(
                                                balance_assertion,
                                                &entry_context,
                                            )?;
                                        if !(bacommodity == commodity
                                            && account_balance
                                                .commodity
                                                .get(&commodity)
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
                                    let (newsum, commodity) = evaluator::eval_and_normalize_amount(
                                        assignment,
                                        &entry_context,
                                    )?;
                                    let value = newsum
                                        - account_balance
                                            .commodity
                                            .get(&commodity)
                                            .unwrap_or(&Decimal::ZERO);
                                    (value, commodity, None)
                                }
                            };
                            let payee = posting.metadata.remove("payee").unwrap_or(payee.clone());

                            if let Some((lot_total, lot_commodity)) = lot_pricing {
                                *(transaction_state.0.entry(lot_commodity).or_default()) +=
                                    lot_total;
                            } else {
                                *(transaction_state.0.entry(commodity.clone()).or_default()) +=
                                    value;
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
                        if transaction_state.0.values().any(|value| !value.is_zero()) {
                            return Err(ElaborationError::TransactionDoesNotBalance(
                                transaction_state,
                            ));
                        }
                    }

                    // Finally, update account balances
                    for posting in resolved_postings.iter() {
                        let balances = state
                            .account_balances
                            .entry(posting.account.clone())
                            .or_default();
                        for (commodity, delta) in posting.amount.0.iter() {
                            *(balances.commodity.entry(commodity.clone()).or_default()) += delta;
                        }
                    }

                    let running_state = state.clone().minify();

                    transactions.push(ResolvedTransaction {
                        date: transaction.date,
                        secondary_date: transaction.secondary_date,
                        state: transaction.state.into(),
                        code: transaction.code,
                        description: transaction.description,
                        tags: transaction.tags,
                        metadata: transaction.metadata,
                        postings: resolved_postings,
                        running_state,
                    });
                }
                resolution::Entry::Price(()) => todo!(),
                resolution::Entry::Assertion(()) => todo!(),
            }
        }

        Ok(Journal { transactions })
    }
}

mod evaluator {
    use std::collections::BTreeMap;

    use rust_decimal::Decimal;

    use crate::{ast, resolution};

    use super::{ElaborationError, EvaluationError};

    pub fn eval_and_normalize_amount(
        val: ast::ValueExpr,
        eval_context: &resolution::Context,
    ) -> Result<(Decimal, String), ElaborationError> {
        match eval(val)? {
            ast::ValueExpr::Amount { value, commodity } => {
                let commodity = if let Some(commodity) = commodity {
                    eval_context
                        .commodity_aliases
                        .get(&commodity)
                        .unwrap_or(&commodity)
                        .clone()
                } else {
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

    fn eval(val: ast::ValueExpr) -> Result<ast::ValueExpr, EvaluationError> {
        match val {
            a @ ast::ValueExpr::Amount { .. } => Ok(a),
            s @ ast::ValueExpr::Str(_) => Ok(s),
            o @ ast::ValueExpr::Object(_) => Ok(o),
            ast::ValueExpr::Unary { op, expr } => match eval(*expr)? {
                ast::ValueExpr::Amount { value, commodity } => match op {
                    ast::Op::Sub => Ok(ast::ValueExpr::Amount {
                        value: -value,
                        commodity,
                    }),
                    ast::Op::Add => Ok(ast::ValueExpr::Amount { value, commodity }),
                    _ => Err(EvaluationError::UnaryMultiplyOrDivide),
                },
                val => Err(EvaluationError::UnaryOnNonAmount(val)),
            },
            ast::ValueExpr::Binary { lhs, rhs, op } => {
                match (eval(*lhs)?, eval(*rhs)?) {
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
                    // Case where someone wrote "-$123" or "$-123"
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
                ("scrub", [arg]) => eval(arg.clone()),
                ("account", [_account]) => Ok(ast::ValueExpr::Object(BTreeMap::from([(
                    "total".into(),
                    ast::ValueExpr::Amount {
                        value: Decimal::ZERO,
                        commodity: Some("$".into()),
                    },
                )]))),
                _ => Err(EvaluationError::UnknownFunctionArgs((name, args))),
            },
            c @ ast::ValueExpr::Commodity(_) => Ok(c),
            ast::ValueExpr::Typed {
                expr,
                commodity: new_commodity,
            } => match eval(*expr)? {
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
            ast::ValueExpr::Access { expr, field } => match eval(*expr)? {
                ast::ValueExpr::Object(map) => map
                    .get(&field)
                    .cloned()
                    .ok_or(EvaluationError::NoSuchField(field)),
                val => Err(EvaluationError::FieldAccessTypeError(val)),
            },
        }
    }
}
