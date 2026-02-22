use std::{collections::BTreeMap, fmt::Display};

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

#[derive(Debug)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

#[derive(Debug)]
pub enum Entry {
    Transaction(Transaction),
    Directive(Directive),
    Comment(String),
}

#[derive(Debug)]
pub enum Directive {
    Commodity {
        name: String,
        notes: Vec<String>,
        items: Vec<CommodityItem>,
    },
    Account {
        name: String,
        notes: Vec<String>,
        items: Vec<AccountItem>,
    },
    Unknown(String),
    Alias {
        alias: String,
        account: String,
    },
}

#[derive(Clone, Debug)]
pub enum CommodityItem {
    Alias(String),
    Format(String), // Format strings are usually "settings" literals
    NoMarket,
    Default,
    Note(String),
    Unknown(String, Option<String>),
}

#[derive(Clone, Debug)]
pub enum AccountItem {
    Alias(String),
    Note(String),
    Unknown(String, Option<String>),
}

#[derive(Clone, Default, Debug)]
pub struct Date {
    pub year: Option<i32>,
    pub month: u32,
    pub date: u32,
}

impl From<NaiveDate> for Date {
    fn from(value: NaiveDate) -> Self {
        Self {
            year: Some(value.year()),
            month: value.month0() + 1,
            date: value.day0() + 1,
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct Transaction {
    pub date: Date,
    pub secondary_date: Option<Date>,
    pub state: TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub notes: Vec<String>,
    pub postings: Vec<Posting>,
}

impl Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(year) = self.date.year {
            write!(f, "{year:04}-")?;
        }
        write!(f, "{:02}-{:02}", self.date.month, self.date.date)?;

        if let Some(ref date) = self.secondary_date {
            write!(f, "=")?;
            if let Some(year) = date.year {
                write!(f, "{year:04}-")?;
            }
            write!(f, "{:02}-{:02}", date.month, date.date)?;
        }

        match self.state {
            TransactionState::Uncleared => {}
            TransactionState::Pending => write!(f, " !")?,
            TransactionState::Cleared => write!(f, " *")?,
        }

        if let Some(ref code) = self.code {
            write!(f, " ({code})")?;
        }

        writeln!(f, " {}", self.description)?;

        for note in self.notes.iter() {
            writeln!(f, "  ; {note}")?;
        }

        for posting in self.postings.iter() {
            posting.fmt(f)?;
        }

        Ok(())
    }
}

#[derive(Clone, Default, Debug)]
pub struct Posting {
    pub account: String,
    pub amount: Option<AmountDetails>,
    pub state: TransactionState,
    pub notes: Vec<String>,
}

impl Posting {
    pub fn new<S: Into<String>>(account: S) -> Self {
        Self {
            account: account.into(),
            amount: None,
            state: TransactionState::Uncleared,
            notes: vec![],
        }
    }

    pub fn with_note<S: Into<String>>(mut self, note: S) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_amount<A: Into<AmountDetails>>(mut self, amount: A) -> Self {
        self.amount = Some(amount.into());
        self
    }
}

impl Display for Posting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "  ")?;
        match self.state {
            TransactionState::Uncleared => {}
            TransactionState::Pending => write!(f, "! ")?,
            TransactionState::Cleared => write!(f, "* ")?,
        }

        write!(f, "{}", self.account)?;

        if let Some(ref amount) = self.amount {
            write!(f, "  {amount}")?;
        }
        writeln!(f)?;

        for note in self.notes.iter() {
            writeln!(f, "  ; {note}")?;
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum AmountDetails {
    Amount {
        value: ValueExpr,
        lot_pricing: Option<LotPricing>,
        balance_assertion: Option<ValueExpr>,
    },
    BalanceAssignment(ValueExpr),
}

impl<I: Into<ValueExpr>> From<I> for AmountDetails {
    fn from(value: I) -> Self {
        AmountDetails::Amount {
            value: value.into(),
            lot_pricing: None,
            balance_assertion: None,
        }
    }
}

impl Display for AmountDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmountDetails::Amount {
                value,
                lot_pricing,
                balance_assertion,
            } => {
                write!(f, "{value}")?;
                if let Some(lot_pricing) = lot_pricing {
                    match lot_pricing {
                        LotPricing::Unit(value_expr) => write!(f, " @ {value_expr}")?,
                        LotPricing::Total(value_expr) => write!(f, " @@ {value_expr}")?,
                    }
                }
                if let Some(balance_assertion) = balance_assertion {
                    write!(f, " = {balance_assertion}")?;
                }
                Ok(())
            }
            AmountDetails::BalanceAssignment(value) => {
                write!(f, "={value}")
            }
        }
    }
}

impl Display for ValueExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueExpr::Amount { value, commodity } => {
                write!(f, "{value}")?;
                if let Some(commodity) = commodity {
                    write!(f, " {commodity}")?;
                }
                Ok(())
            }
            ValueExpr::Str(s) => write!(f, "\"{s}\""),
            ValueExpr::Unary { op, expr } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{op}{expr}")
            }
            ValueExpr::Binary { lhs, rhs, op } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{lhs} {op} {rhs}")
            }
            ValueExpr::Function { name, args } => {
                write!(f, "{name}(")?;
                let mut args = args.iter();
                if let Some(a) = args.next() {
                    write!(f, "{a}")?;
                }
                for a in args {
                    write!(f, ", {a}")?;
                }
                write!(f, ")")
            }
            ValueExpr::Commodity(c) => write!(f, "{c}"),
            ValueExpr::Typed { expr, commodity } => write!(f, "{expr} {commodity}"),
            ValueExpr::Access { expr, field } => write!(f, "{expr}.{field}"),
            ValueExpr::Object(_) => todo!(),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ValueExpr {
    Object(BTreeMap<String, Self>),
    Amount {
        value: Decimal,
        commodity: Option<String>,
    },
    // For things like: (1 + 2) USD
    Unary {
        op: Op,
        expr: Box<ValueExpr>,
    },
    Binary {
        lhs: Box<ValueExpr>,
        rhs: Box<ValueExpr>,
        op: Op,
    },
    Function {
        name: String,
        args: Vec<ValueExpr>,
    },
    // For commodities that stand alone or are part of a math group
    Commodity(String),
    Typed {
        expr: Box<ValueExpr>,
        commodity: String,
    },
    Str(String),
    Access {
        expr: Box<ValueExpr>,
        field: String,
    },
}

impl ValueExpr {
    pub fn amount(value: Decimal, commodity: String) -> ValueExpr {
        ValueExpr::Amount {
            value,
            commodity: Some(commodity),
        }
    }
}

impl<S: Into<String>> From<(Decimal, S)> for ValueExpr {
    fn from(value: (Decimal, S)) -> Self {
        Self::amount(value.0, value.1.into())
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum LotPricing {
    Unit(ValueExpr),
    Total(ValueExpr),
}

#[derive(Clone, Debug, Default)]
pub enum TransactionState {
    #[default]
    Uncleared,
    Pending,
    Cleared,
}
