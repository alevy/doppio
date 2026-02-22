use crate::ast::*;
use pest::Parser as _;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::PrattParser;
use pest_derive::Parser;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::LazyLock; // Or once_cell

#[derive(Parser)]
#[grammar = "ledger.pest"]
pub struct LedgerParser;

pub struct Parser<F: Fn(&str) -> String> {
    pub openner: F,
    pub base_path: PathBuf,
}

impl<F: Fn(&str) -> String> Parser<F> {
    pub fn parse(&mut self, input: &str) -> Result<Journal, pest::error::Error<Rule>> {
        let pairs = LedgerParser::parse(Rule::journal, input)?;
        let mut entries = Vec::new();

        for pair in pairs.into_iter().next().unwrap().into_inner() {
            match pair.as_rule() {
                Rule::transaction => {
                    entries.push(Entry::Transaction(parse_transaction(pair)));
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::commodity_directive => {
                    entries.push(Entry::Directive(parse_commodity_directive(pair)));
                }
                Rule::account_directive => {
                    entries.push(Entry::Directive(parse_account_directive(pair)));
                }
                Rule::alias_directive => {
                    entries.push(Entry::Directive(parse_alias_directive(pair)));
                }
                Rule::include_directive => {
                    let include_path = self.base_path.join(pair.into_inner().as_str());
                    let new_input = (self.openner)(&include_path.as_os_str().to_str().unwrap());
                    let new_base_path = include_path
                        .parent()
                        .map(|p| self.base_path.join(p))
                        .unwrap_or(self.base_path.clone());
                    let old_base_path = std::mem::replace(&mut self.base_path, new_base_path);
                    entries.append(&mut self.parse(&new_input)?.entries);
                    let _ = std::mem::replace(&mut self.base_path, old_base_path);
                }
                _ => {}
            }
        }

        Ok(Journal { entries })
    }
}

pub fn parse_ledger(input: &str) -> Result<Journal, pest::error::Error<Rule>> {
    Parser {
        openner: |_| String::new(),
        base_path: PathBuf::new(),
    }
    .parse(input)
}

fn parse_alias_directive(pair: Pair<Rule>) -> Directive {
    let mut pairs = pair.into_inner();
    let alias = pairs.next().unwrap().as_str().trim().to_string();
    let account = pairs.next().unwrap().as_str().trim().to_string();
    Directive::Alias { alias, account }
}

fn parse_account_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap().as_str().to_string();
    let mut notes = Vec::new();
    let mut items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::account_item => {
                items.push(parse_account_item(p));
            }
            _ => {}
        }
    }

    Directive::Account { name, notes, items }
}

fn parse_commodity_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap().as_str().to_string();
    let mut notes = Vec::new();
    let mut items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::commodity_item => {
                items.push(parse_commodity_item(p));
            }
            _ => {}
        }
    }

    Directive::Commodity { name, notes, items }
}

fn parse_account_item(pair: Pair<Rule>) -> AccountItem {
    let mut inner = pair.into_inner();
    let key_pair = inner.next().unwrap();
    let key = key_pair.as_str();
    // Look for value and trailing note
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::account_val {
            val = Some(p.as_str().trim().to_string())
        }
    }

    match key {
        "alias" => AccountItem::Alias(val.unwrap_or_default()),
        "note" => AccountItem::Note(val.unwrap_or_default()),
        _ => AccountItem::Unknown(key.to_string(), val),
    }
}

fn parse_commodity_item(pair: Pair<Rule>) -> CommodityItem {
    let mut inner = pair.into_inner();
    let key_pair = inner.next().unwrap();
    let key = key_pair.as_str();
    // Look for value and trailing note
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::commodity_val {
            val = Some(p.as_str().trim().to_string())
        }
    }

    match key {
        "alias" => CommodityItem::Alias(val.unwrap_or_default()),
        "format" => CommodityItem::Format(val.unwrap_or_default()),
        "nomarket" => CommodityItem::NoMarket,
        "default" => CommodityItem::Default,
        _ => CommodityItem::Unknown(key.to_string(), val),
    }
}

fn parse_date(pairs: &mut Pairs<Rule>) -> Date {
    let mut year: Option<i32> = None;

    let mut p = pairs.next().unwrap();
    if let Rule::year = p.as_rule() {
        year = Some(p.as_str().parse().unwrap());
        p = pairs.next().unwrap();
    }

    let month = p.as_str().parse().unwrap();
    let date = p.as_str().parse().unwrap();

    Date { year, month, date }
}

fn parse_transaction(pair: Pair<Rule>) -> Transaction {
    let mut inner = pair.into_inner();
    let header_pair = inner.next().unwrap();
    let mut postings = Vec::new();
    let mut notes = Vec::new();

    // Process remainder of transaction
    for p in inner {
        match p.as_rule() {
            Rule::transaction_note => {
                // Get the inner note rule
                if let Some(note_pair) = p.into_inner().next() {
                    notes.push(note_pair.into_inner().as_str().trim().to_string());
                }
            }
            Rule::posting => {
                postings.push(parse_posting(p));
            }
            _ => {}
        }
    }
    let mut header = header_pair.into_inner();
    let date = parse_date(&mut header.next().unwrap().into_inner());

    let mut secondary_date = None;
    let mut state = TransactionState::Uncleared;
    let mut code = None;
    let mut description = String::new();

    for p in header {
        match p.as_rule() {
            Rule::date => secondary_date = Some(parse_date(&mut p.into_inner())),
            Rule::state => state = parse_state(p.as_str()),
            Rule::code => {
                // Remove parentheses from code
                let s = p.as_str();
                code = Some(s[1..s.len() - 1].to_string());
            }
            Rule::description => description = p.as_str().trim().to_string(),
            Rule::note => notes.push(p.as_str().trim().to_string()),
            _ => {}
        }
    }

    Transaction {
        date,
        secondary_date,
        state,
        code,
        description,
        notes,
        postings,
    }
}

fn parse_posting(pair: Pair<Rule>) -> Posting {
    let inner = pair.into_inner();
    let mut state = TransactionState::Uncleared;
    let mut account = String::new();
    let mut amount = None;
    let mut notes = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::state => state = parse_state(p.as_str()),
            Rule::account => account = p.as_str().trim().to_string(),
            Rule::amount_logic => amount = Some(parse_amount_logic(p)),
            Rule::note => notes.push(p.as_str().trim().to_string()),
            Rule::posting_note => {
                if let Some(note_pair) = p.into_inner().next() {
                    notes.push(note_pair.into_inner().as_str().trim().to_string());
                }
            }
            _ => {}
        }
    }

    Posting {
        account,
        amount,
        state,
        notes,
    }
}

fn parse_amount_logic(pair: Pair<Rule>) -> AmountDetails {
    let p = pair.into_inner().next().unwrap();
    match p.as_rule() {
        Rule::value_logic => {
            let inner = p.into_inner();

            let mut value = None;
            let mut lot_pricing = None;
            let mut balance_assertion = None;

            for p in inner {
                match p.as_rule() {
                    Rule::value_expr => {
                        value = Some(parse_expr(p));
                    }
                    Rule::lot_price => {
                        let s = p.as_str();
                        let inner_val = parse_expr(p.into_inner().next().unwrap());
                        if s.starts_with("@@") {
                            lot_pricing = Some(LotPricing::Total(inner_val));
                        } else {
                            lot_pricing = Some(LotPricing::Unit(inner_val));
                        }
                    }
                    Rule::assertion => {
                        // Now parsing as a ValueExpr
                        let inner_expr_pair = p.into_inner().next().unwrap();
                        balance_assertion = Some(parse_expr(inner_expr_pair));
                    }
                    _ => unreachable!(),
                }
            }
            AmountDetails::Amount {
                value: value.unwrap(),
                lot_pricing,
                balance_assertion,
            }
        }
        Rule::assertion => {
            let inner_expr_pair = p.into_inner().next().unwrap();
            AmountDetails::BalanceAssignment(parse_expr(inner_expr_pair))
        }
        _ => unreachable!(),
    }
}

static PRATT_PARSER: LazyLock<PrattParser<Rule>> = LazyLock::new(|| {
    use Rule::*;
    use pest::pratt_parser::{Assoc::*, Op};

    PrattParser::new()
        .op(Op::infix(add, Left) | Op::infix(sub, Left))
        .op(Op::infix(mul, Left) | Op::infix(div, Left))
        .op(Op::prefix(prefix_op))
});

fn parse_expr(pair: Pair<Rule>) -> ValueExpr {
    let mut inner = pair.into_inner();
    let expr_pair = inner.next().expect("Empty value_expr");
    let mut ast = run_pratt(expr_pair.into_inner());

    // Check for trailing commodity (e.g., '(1+2) USD')
    if let Some(comm_pair) = inner.next() {
        ast = ValueExpr::Typed {
            expr: Box::new(ast),
            commodity: comm_pair.as_str().to_string(),
        };
    }
    ast
}

fn run_pratt(pairs: pest::iterators::Pairs<Rule>) -> ValueExpr {
    PRATT_PARSER
        .map_primary(|pair| match pair.as_rule() {
            Rule::term => run_pratt(pair.into_inner()),
            Rule::primary => {
                let mut inner = pair.into_inner();
                // Get the base atom (Amount, Function, String, etc.)
                // Note: base_primary is silent, so we get its child directly
                let base_pair = inner.next().expect("Primary must have a base");

                // Recursively parse the base using run_pratt
                // We wrap it in a single-item iterator to reuse our logic
                let mut ast = run_pratt(pest::iterators::Pairs::single(base_pair));

                // 3. Fold any dot-accessors into the AST
                for access in inner {
                    if access.as_rule() == Rule::access {
                        let field = access.into_inner().next().unwrap().as_str().to_string();
                        ast = ValueExpr::Access {
                            expr: Box::new(ast),
                            field,
                        };
                    }
                }
                ast
            }
            Rule::amount => {
                let mut inner = pair.into_inner();
                let first = inner.next().unwrap();
                match first.as_rule() {
                    Rule::commodity => {
                        let comm = first.as_str().to_string();
                        let val_str = inner.next().unwrap().as_str();
                        ValueExpr::Amount {
                            value: clean_parse_decimal(val_str),
                            commodity: Some(comm),
                        }
                    }
                    Rule::number => {
                        let val = clean_parse_decimal(first.as_str());
                        let comm = inner.next().map(|c| c.as_str().to_string());
                        ValueExpr::Amount {
                            value: val,
                            commodity: comm,
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Rule::commodity => ValueExpr::Commodity(pair.as_str().to_string()),
            Rule::function_call => {
                let mut inner = pair.into_inner();
                let name = inner.next().unwrap().as_str().to_string();
                // Function args are Rule::expr. run_pratt handles Rule::expr pairs.
                let args = inner.map(|p| run_pratt(p.into_inner())).collect();
                ValueExpr::Function { name, args }
            }
            Rule::expr => run_pratt(pair.into_inner()),
            Rule::string => {
                let s = pair.as_str();
                // Strip the first and last characters (the quotes)
                ValueExpr::Str(s[1..s.len() - 1].to_string())
            }
            _ => unreachable!("{:?}", pair.as_rule()),
        })
        .map_prefix(|op, expr| ValueExpr::Unary {
            op: if op.as_str() == "-" { Op::Sub } else { Op::Add },
            expr: Box::new(expr),
        })
        .map_infix(|lhs, op, rhs| {
            let op = match op.as_rule() {
                Rule::add => Op::Add,
                Rule::sub => Op::Sub,
                Rule::mul => Op::Mul,
                Rule::div => Op::Div,
                _ => unreachable!(),
            };
            ValueExpr::Binary {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op,
            }
        })
        .parse(pairs)
}

fn clean_parse_decimal(s: &str) -> Decimal {
    let cleaned = s.replace(',', "");
    cleaned.parse().unwrap_or(Decimal::ZERO)
}

fn parse_state(s: &str) -> TransactionState {
    match s {
        "*" => TransactionState::Cleared,
        "!" => TransactionState::Pending,
        _ => TransactionState::Uncleared,
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::dec;

    use super::*;

    #[test]
    fn test_simple_transaction() {
        let input =
            "2023-01-01 * (123) Grocery Store\n  Expenses:Food  $50.00\n  Assets:Checking\n";
        let journal = parse_ledger(input).unwrap();

        assert_eq!(journal.entries.len(), 1);
        if let Entry::Transaction(tx) = &journal.entries[0] {
            assert_eq!(tx.description, "Grocery Store");
            assert_eq!(tx.code, Some("123".to_string()));
            assert!(matches!(tx.state, TransactionState::Cleared));
            assert_eq!(tx.postings.len(), 2);
            assert_eq!(tx.postings[0].account, "Expenses:Food");
            assert_eq!(
                tx.postings[0].amount,
                Some(AmountDetails::Amount {
                    value: ValueExpr::Amount {
                        value: dec!(50.00),
                        commodity: Some("$".into()),
                    },
                    lot_pricing: None,
                    balance_assertion: None,
                })
            );
            assert_eq!(tx.postings[1].account, "Assets:Checking");
            assert!(tx.postings[1].amount.is_none());
        } else {
            panic!("Expected a transaction");
        }
    }

    #[test]
    fn test_lot_and_assertion() {
        let input = "2023-01-01 * Stock Purchase\n  Assets:Brokerage  10 AAPL @ $150.00 = $1500.00\n  Assets:Checking\n";
        let journal = parse_ledger(input).expect("Should parse successfully");

        if let Entry::Transaction(ref tx) = journal.entries[0] {
            let p = &tx.postings[0];
            let details = p.amount.as_ref().expect("Should have amount details");

            assert_eq!(
                details,
                &AmountDetails::Amount {
                    value: ValueExpr::Amount {
                        value: dec!(10),
                        commodity: Some("AAPL".into()),
                    },
                    lot_pricing: Some(LotPricing::Unit(ValueExpr::Amount {
                        commodity: Some("$".into()),
                        value: dec!(150.00)
                    })),
                    balance_assertion: Some(ValueExpr::Amount {
                        value: dec!(1500.00),
                        commodity: Some("$".to_string()),
                    })
                }
            );
        }
    }

    #[test]
    fn test_notes_and_comments() {
        let input = "
; Top level comment
2023-01-01 Transaction with notes
  ; Header note
  Expenses:Rent  $1000
  ; Posting note
  Assets:Checking
";
        let journal = parse_ledger(input).unwrap();

        // Entry 0 is an empty line (optional depending on grammar strictness)
        // Entry 1 is the comment
        // Entry 2 is the transaction
        let tx = journal
            .entries
            .iter()
            .find_map(|e| {
                if let Entry::Transaction(t) = e {
                    Some(t)
                } else {
                    None
                }
            })
            .expect("Transaction not found");

        assert_eq!(tx.notes[0], "Header note");
        assert_eq!(tx.postings[0].notes[0], "Posting note");
    }

    #[test]
    fn test_invalid_date() {
        let input = "23-01-01 * Missing Year Century\n  Expenses:Food  $10.00\n  Assets:Cash\n";
        let result = parse_ledger(input);
        assert!(result.is_err(), "Should fail due to strict date format");
    }

    #[test]
    fn test_complex_math_and_commas() {
        // 1. Thousand separators
        // 2. Nested parentheses
        // 3. Precedence: (1,000 + 200) * 2 = 2,400
        let input = "2023-01-01 * Math Test
    Expenses:Food  (1,000.00 + 200) * 2 USD
    Assets:Cash    $-1,234.56
";
        let journal = parse_ledger(input).unwrap();
        let tx = match &journal.entries[0] {
            Entry::Transaction(t) => t,
            _ => panic!("Expected transaction"),
        };

        // Verify first posting (Complex Math)
        let p1 = &tx.postings[0];
        if let Some(details) = &p1.amount {
            // We expect a Binary expression at the top level
            assert!(matches!(
                details,
                AmountDetails::Amount {
                    value: ValueExpr::Binary { .. },
                    ..
                }
            ));
        }

        // Verify second posting (Negative with Commas)
        let p2 = &tx.postings[1];
        if let Some(details) = &p2.amount {
            // This is interesting: depending on whether $-1234 or -$1234 is used,
            // it might be a Unary(Amount) or an Amount with a negative number.
            // Current grammar for `amount` + `prefix_op` makes this a Unary(Amount).
            assert!(matches!(
                details,
                AmountDetails::Amount {
                    value: ValueExpr::Binary { .. },
                    ..
                }
            ));
        }
    }

    #[test]
    fn test_function_calls() {
        let input = "2023-01-01 * Func Test
    Expenses:Travel  market(100, 2023-01-01)
    Assets:Checking
";
        let journal = parse_ledger(input).unwrap();
        let tx = match &journal.entries[0] {
            Entry::Transaction(t) => t,
            _ => panic!("Expected transaction"),
        };

        let p1 = &tx.postings[0];
        match &p1.amount {
            Some(AmountDetails::Amount {
                value: ValueExpr::Function { name, args },
                ..
            }) => {
                assert_eq!(name, "market");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected a function call, got {:?}", p1.amount),
        }
    }

    #[test]
    fn test_just_math() {
        let input = "(100 + 20) * 5";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        // next() returns the Pair<Rule::value_expr>
        let expr = parse_expr(pairs.next().unwrap());
        println!("{:?}", expr);
    }

    #[test]
    fn test_math_with_commodity() {
        let input = "(100 + 20) USD";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());
        assert!(matches!(expr, ValueExpr::Typed { .. }));
    }

    #[test]
    fn test_comma_number() {
        let input = "1,234.56";
        let pairs = LedgerParser::parse(Rule::number, input).unwrap();
        assert_eq!(clean_parse_decimal(pairs.as_str()), dec!(1234.56));
    }
}

#[cfg(test)]
mod directed_tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn test_number_commodity_variants() {
        let cases = vec![
            ("$1000", dec!(1000), Some("$")),
            ("1000 USD", dec!(1000), Some("USD")),
        ];

        for (input, expected_val, expected_comm) in cases {
            let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
            let expr = parse_expr(pairs.next().unwrap());
            if let ValueExpr::Amount { value, commodity } = expr {
                assert_eq!(value, expected_val);
                assert_eq!(commodity, expected_comm.map(|s| s.to_string()));
            } else {
                panic!("Expected Amount, got {:?}", expr);
            }
        }

        // Handle the negative case separately as it's a Unary tree
        let input = "-1,234.56 BTC";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());
        assert!(matches!(expr, ValueExpr::Unary { op: Op::Sub, .. }));
    }

    #[test]
    fn test_posting_variants() {
        let input = "2023-01-01 Transaction
    Expenses:NoAmount
    Expenses:SimpleAmount  $100
    Expenses:Expression    (100 + 100) USD";

        // Parse specifically as a transaction
        let mut pairs = LedgerParser::parse(Rule::transaction, input).unwrap();
        let tx_pair = pairs.next().unwrap();
        let tx = parse_transaction(tx_pair);

        assert_eq!(tx.postings.len(), 3);
    }

    #[test]
    fn test_balance_assignment() {
        let input = "2024-12-17 Opening Balance
        Assets:Bank:Checking    =$21,966.08
        Equity:Opening Balances";

        let journal = parse_ledger(input).expect("Should parse balance assignment");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!()
        };

        let p = &tx.postings[0];
        assert_eq!(p.account, "Assets:Bank:Checking");

        let details = p.amount.as_ref().expect("Should have amount details");
        assert_eq!(
            *details,
            AmountDetails::BalanceAssignment(ValueExpr::Amount {
                value: dec!(21966.08),
                commodity: Some("$".into())
            })
        );
    }

    #[test]
    fn test_commodity_directive_block() {
        let input = "commodity BTC
    ; The primary crypto
    alias Bitcoin
    format 1,000.00000000 BTC
    nomarket
    default
";
        let journal = parse_ledger(input).expect("Should parse commodity directive");

        if let Entry::Directive(Directive::Commodity { name, notes, items }) = &journal.entries[0] {
            assert_eq!(name, "BTC");
            assert_eq!(notes[0], "The primary crypto");
            assert_eq!(items.len(), 4);

            assert!(matches!(items[0], CommodityItem::Alias(_)));
            assert!(matches!(items[1], CommodityItem::Format(_)));
            assert!(matches!(items[2], CommodityItem::NoMarket));
            assert!(matches!(items[3], CommodityItem::Default));
        } else {
            panic!("Expected a Commodity Directive");
        }
    }

    #[test]
    fn test_string_in_function() {
        let input = "account(\"Assets:Bank:Checking\")";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());

        if let ValueExpr::Function { name, args } = expr {
            assert_eq!(name, "account");
            match &args[0] {
                ValueExpr::Str(s) => assert_eq!(s, "Assets:Bank:Checking"),
                _ => panic!("Expected string argument"),
            }
        } else {
            panic!("Expected function call");
        }
    }

    #[test]
    fn test_field_access() {
        let input = "account(\"Assets:Bank\").total.quantity";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());

        if let ValueExpr::Access { expr: inner, field } = expr {
            assert_eq!(field, "quantity");
            if let ValueExpr::Access {
                field: inner_field, ..
            } = *inner
            {
                assert_eq!(inner_field, "total");
            } else {
                panic!("Expected nested access");
            }
        } else {
            panic!("Expected field access, got {:?}", expr);
        }
    }
}
