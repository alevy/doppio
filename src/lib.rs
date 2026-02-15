use std::collections::{BTreeMap, BTreeSet};

use ast::CommodityItem;
use rust_decimal::Decimal;

pub mod ast;
pub mod parser;

pub struct Account {
    pub name: String,
    pub note: Option<String>,
    pub balances: BTreeMap<Option<String>, Decimal>,
}

pub struct Transaction {
    pub date: chrono::NaiveDate,
    pub payee: String,
    pub postings: Vec<Posting>,
}

pub struct Posting {
    pub account: String,
    pub amount: Decimal,
    pub currency: String,
}

pub struct Commodity {
    pub name: String,
    pub aliases: BTreeSet<String>,
    pub notes: Vec<String>,
}

#[derive(Default)]
pub struct Journal {
    pub accounts: BTreeMap<String, Account>,
    pub commodities: BTreeMap<String, Commodity>,
    pub transactions: Vec<Transaction>,
}

impl Journal {
    fn eval(&self, val: ast::ValueExpr) -> ast::ValueExpr {
        match val {
            a@ast::ValueExpr::Amount { .. } => a,
            s@ast::ValueExpr::Str(_) => s,
            o@ast::ValueExpr::Object(_) => o,
            ast::ValueExpr::Unary { op, expr } => {
                match self.eval(*expr) {
                    ast::ValueExpr::Amount { value, commodity } => {
                        match op {
                            ast::Op::Sub => ast::ValueExpr::Amount {
                                value: -value,
                                commodity
                            },
                            ast::Op::Add => ast::ValueExpr::Amount {
                                value,
                                commodity
                            },
                            _ => panic!("Can't multiple or divide in a unary operation"),
                        }
                    },
                    _ => panic!("Can't perform unary operations on a non-amount"),
                }
            },
            ast::ValueExpr::Binary { lhs, rhs, op } => {
                match (self.eval(*lhs), self.eval(*rhs)) {
                    (ast::ValueExpr::Commodity(c), ast::ValueExpr::Amount { value, commodity: None }) |
                    (ast::ValueExpr::Amount { value, commodity: None }, ast::ValueExpr::Commodity(c)) => {
                        match op {
                            ast::Op::Sub => ast::ValueExpr::Amount {
                                value: -value,
                                commodity: Some(c),
                            },
                            ast::Op::Add => ast::ValueExpr::Amount {
                                value,
                                commodity: Some(c),
                            },
                            _ => panic!("Can't multiple or divide in a unary operation"),
                        }

                    },
                    (ast::ValueExpr::Amount { value: v1, commodity: c }, ast::ValueExpr::Amount { value: v2, commodity: None }) |
                    (ast::ValueExpr::Amount { value: v1, commodity: None }, ast::ValueExpr::Amount { value: v2, commodity: c }) => {
                        match op {
                            ast::Op::Add =>
                        ast::ValueExpr::Amount { value: v1 + v2, commodity: c },
                            ast::Op::Sub =>
                        ast::ValueExpr::Amount { value: v1 - v2, commodity: c },
                            ast::Op::Mul =>
                        ast::ValueExpr::Amount { value: v1 * v2, commodity: c },
                            ast::Op::Div =>
                        ast::ValueExpr::Amount { value: v1 / v2, commodity: c },
                        }
                    },
                    (ast::ValueExpr::Amount { value: v1, commodity: c }, ast::ValueExpr::Amount { value: v2, commodity: c2 }) if c == c2 => {
                        match op {
                            ast::Op::Add =>
                        ast::ValueExpr::Amount { value: v1 + v2, commodity: c },
                            ast::Op::Sub =>
                        ast::ValueExpr::Amount { value: v1 - v2, commodity: c },
                            ast::Op::Mul =>
                        ast::ValueExpr::Amount { value: v1 * v2, commodity: c },
                            ast::Op::Div =>
                        ast::ValueExpr::Amount { value: v1 / v2, commodity: c },
                        }
                    }
                    (a, b) => panic!("Can only perform binary operations on amounts of like commodity {a:?} {b:?} {op:?}"),
                }
            },
            ast::ValueExpr::Function { name, args } => {
                match (name.as_str(), args.as_slice()) {
                    ("scrub", [arg]) => self.eval(arg.clone()),
                    ("account", [_account]) => {
                        ast::ValueExpr::Object(BTreeMap::from([("total".into(), ast::ValueExpr::Amount {
                            value: Decimal::ZERO,
                            commodity: Some("$".into())
                        })]))
                    },
                    _ => panic!("{name} {args:?}"),
                }
            },
            c@ast::ValueExpr::Commodity(_) => c,
            ast::ValueExpr::Typed { expr, commodity: new_commodity } => {
                match self.eval(*expr) {
                    ast::ValueExpr::Amount { value, commodity } if commodity.is_none() || commodity.as_ref() == Some(&new_commodity) => {
                        ast::ValueExpr::Amount { value, commodity: Some(new_commodity) }
                    },
                    _ => panic!("Can only assign a commodity to an amount with no or the same commodity"),
                }
            },
            ast::ValueExpr::Access { expr, field } => {
                match self.eval(*expr) {
                    ast::ValueExpr::Object(map) => {
                        map.get(&field).expect("No such field").clone()
                    },
                    _ => panic!("Can only access fields on objects"),
                }
            },
        }
    }

    pub fn resolve_commodity(&self, alias: &String) -> Option<&Commodity> {
        self.commodities.values().find(|t| t.aliases.contains(alias))
    }

    pub fn compile(ast: &ast::Journal) -> Result<Self, ()> {
        let mut result: Journal = Default::default();

        for entry in ast.entries.iter() {
            match entry {
                ast::Entry::Directive(directive) => {
                    match directive {
                        ast::Directive::Commodity { name, notes, items } => {
                            let commodity = result.commodities.entry(name.clone()).or_insert(Commodity {
                                name: name.clone(),
                                notes: vec![],
                                aliases: Default::default(),
                            });
                            commodity.notes.append(&mut notes.clone());
                            for item in items {
                                if let CommodityItem::Alias(alias) = item {
                                    commodity.aliases.insert(alias.clone());
                                }
                            }
                        }
                        ast::Directive::Account(account) => {
                            result.accounts.insert(account.clone(), Account {
                                name: account.clone(),
                                note: None,
                                balances: Default::default(),
                            });
                        },
                        ast::Directive::Unknown(_) => {
                            // TODO: warning
                        },
                    }
                }
                ast::Entry::Comment(_comment) => {}
                ast::Entry::Transaction(transaction) => {
                    let mut running_sum: BTreeMap<Option<String>, Decimal> = BTreeMap::new();
                    for posting in transaction.postings.iter() {
                        result
                            .accounts
                            .entry(posting.account.clone())
                            .or_insert(Account {
                                name: posting.account.clone(),
                                note: None,
                                balances: Default::default(),
                            });

                        if let Some(ref amount) = posting.amount {
                            if let Some(ref value) = amount.value {
                                match result.eval(value.clone()) {
                                    ast::ValueExpr::Amount { value, commodity } => {
                                        let commodity = if let Some(c) = commodity.as_ref() && let Some(target) = result.resolve_commodity(c) {
                                            Some(target.name.clone())
                                        } else {
                                            commodity
                                        };
                                        *(running_sum.entry(commodity.clone()).or_default()) += value;
                                        let balance = result.accounts.entry(posting.account.clone()).or_insert_with(|| Account {
                                            name: posting.account.clone(),
                                            note: None,
                                            balances: Default::default(),
                                        }).balances.entry(commodity).or_default();
                                        *balance += value;
                                    },
                                    _ => panic!("WTF!"),
                                }
                            } else if let Some(ref balance) = amount.balance_assertion {
                                match result.eval(balance.clone()) {
                                    ast::ValueExpr::Amount { value, commodity } => {
                                        let commodity = if let Some(c) = commodity.as_ref() && let Some(target) = result.resolve_commodity(c) {
                                            Some(target.name.clone())
                                        } else {
                                            commodity
                                        };
                                        let balance = result.accounts.entry(posting.account.clone()).or_insert_with(|| Account {
                                            name: posting.account.clone(),
                                            note: None,
                                            balances: Default::default(),
                                        }).balances.entry(commodity.clone()).or_default();
                                        let orig = *balance;
                                        let diff = value - orig;
                                        *balance = value;
                                        *(running_sum.entry(commodity).or_default()) += diff;
                                    },
                                    _ => panic!("WTF!"),
                                }
                            }
                            }
                        }

                    let mut bare_postings = transaction.postings.iter().filter(|p| p.amount.is_none());
                    if let Some(p) = bare_postings.next() {
                        let account = result.accounts.entry(p.account.clone()).or_insert_with(|| Account {
                            name: p.account.clone(),
                            note: None,
                            balances: Default::default(),
                        });
                        for (commodity, sum) in running_sum {
                            *(account.balances.entry(commodity).or_default()) -= sum;
                        }
                    }
                }
            }
        }
        Ok(result)
    }
}
