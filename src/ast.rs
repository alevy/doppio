use std::collections::BTreeMap;

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
    Account(String),
    Unknown(String),
}

#[derive(Debug)]
pub enum CommodityItem {
    Alias(String),
    Format(String), // Format strings are usually "settings" literals
    NoMarket,
    Default,
    Note(String),
    Unknown(String, Option<String>),
}

#[derive(Debug)]
pub struct Date {
    pub year: Option<u16>,
    pub month: u8,
    pub date: u8,
}

#[derive(Debug)]
pub struct Transaction {
    pub date: Date,
    pub secondary_date: Option<Date>,
    pub state: TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub notes: Vec<String>,
    pub postings: Vec<Posting>,
}

#[derive(Debug)]
pub struct Posting {
    pub account: String,
    pub amount: Option<AmountDetails>,
    pub state: TransactionState,
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub struct AmountDetails {
    pub value: Option<ValueExpr>,
    pub lot_pricing: Option<LotPricing>,
    pub balance_assertion: Option<ValueExpr>,
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

#[derive(Debug)]
pub enum LotPricing {
    Unit(String),
    Total(String),
}

#[derive(Debug, Default)]
pub enum TransactionState {
    #[default]
    Uncleared,
    Pending,
    Cleared,
}
