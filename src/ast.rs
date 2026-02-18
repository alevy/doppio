use std::{collections::BTreeMap, fmt::Display};

use rust_decimal::Decimal;

use crate::parser::{self, Rule};

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
    Account(String),
    Unknown(String),
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

#[derive(Clone, Default, Debug)]
pub struct Date {
    pub year: Option<u16>,
    pub month: u8,
    pub date: u8,
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
            TransactionState::Uncleared => {},
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

impl Display for Posting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "  ")?;
        match self.state {
            TransactionState::Uncleared => {},
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

impl Display for AmountDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmountDetails::Amount { value, lot_pricing, balance_assertion } => {
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
            },
            AmountDetails::BalanceAssignment(value) => {
                write!(f, "={value}")
            },
        }
    }
}

impl Display for ValueExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueExpr::Amount { value, commodity } => {
                write!(f,"{value}")?;
                if let Some(commodity) = commodity {
                    write!(f," {commodity}")?;
                }
                Ok(())
            },
            ValueExpr::Str(s) => write!(f, "\"{s}\""),
            ValueExpr::Unary { op, expr } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{op}{expr}")
            },
            ValueExpr::Binary { lhs, rhs, op } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{lhs} {op} {rhs}")
            },
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
            },
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

impl TryFrom<&str> for Journal {
    type Error = pest::error::Error<Rule>;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        parser::parse_ledger(value)
    }
}
