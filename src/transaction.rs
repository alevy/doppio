use std::{
    collections::HashMap,
    fmt::{Display, Write},
};

use rust_decimal::Decimal;
use winnow::{
    ascii::{alpha1, newline, space0, space1}, combinator::{alt, delimited, dispatch, fail, opt, peek, repeat}, error::{ContextError, StrContext}, stream::AsChar, token::{any, literal, one_of, take, take_till, take_while}, ModalResult, Parser
};

use crate::{Comment, CommentBody, helpers::hard_stop};

#[derive(Clone, Debug)]
pub enum TransactionState {
    Cleared,
    Pending,
    Uncleared,
}

impl TransactionState {
    pub fn parse(input: &mut &str) -> ModalResult<TransactionState> {
        // State is optionally '*' or '!'
        let state = opt(one_of(b"*!"))
            .map(|c| match c {
                Some('*') => TransactionState::Cleared,
                Some('!') => TransactionState::Pending,
                _ => TransactionState::Uncleared,
            })
            .parse_next(input)?;
        space0(input)?;
        Ok(state)
    }
}

impl Display for TransactionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionState::Cleared => write!(f, "* "),
            TransactionState::Pending => write!(f, "! "),
            TransactionState::Uncleared => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub date: chrono::NaiveDateTime,
    pub state: TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub note: Option<CommentBody>,
    pub postings: Vec<Posting>,
}

impl Transaction {
    pub fn literals(&self) -> Vec<String> {
        self.postings
            .iter()
            .filter_map(|posting| {
                if let Posting::Comment(CommentBody::Tags(tags)) = posting {
                    Some(tags)
                } else {
                    None
                }
            })
            .fold(vec![], |mut f, s| {
                f.append(&mut s.clone());
                f
            })
    }

    pub fn value_of<S: AsRef<str>>(&self, key: S) -> Option<&String> {
        self.postings.iter().find_map(|posting| {
            if let Posting::Comment(CommentBody::Value(k, v)) = posting
                && k == key.as_ref()
            {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn metadata(&self) -> HashMap<&String, &String> {
        self.postings
            .iter()
            .filter_map(|posting| {
                if let Posting::Comment(CommentBody::Value(k, v)) = posting {
                    Some((k, v))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn parse(input: &mut &str) -> ModalResult<Transaction> {
        // Date as Y-M-D or Y/M/D
        let date = chrono::NaiveDate::parse_and_remainder(input, "%Y-%m-%d")
            .or_else(|_| chrono::NaiveDate::parse_and_remainder(input, "%Y/%m/%d"))
            .map_or_else(
                |_| {
                    let mut err = ContextError::new();
                    err.push(StrContext::Label("date"));
                    Err(winnow::error::ErrMode::Backtrack(err))
                },
                |(d, i)| {
                    *input = i;
                    Ok(d)
                },
            )?;

        space0(input)?;

        // State is optionally '*' or '!'
        let state = TransactionState::parse(input)?;

        let code = opt(delimited(
            literal("("),
            take_till(1.., |c| c == ')'),
            literal(")"),
        ))
        .parse_next(input)?;

        space0(input)?;

        // description
        let mut description = String::new();
        loop {
            match peek(alt((
                (hard_stop, literal(";")).value(()),
                (newline.value(())),
            )))
            .parse_next(input)
            {
                Err(winnow::error::ErrMode::Backtrack(_)) => {
                    let c = take(1usize).parse_next(input)?;
                    description.push_str(c);
                }
                Err(e) => {
                    return Err(e);
                }
                Ok(_) => break,
            };
        }

        // note
        let note = opt((hard_stop, Comment::parse)).parse_next(input)?;
        if note.is_none() {
            newline(input)?;
        }

        let postings = repeat(.., Posting::parse).parse_next(input)?;

        Ok(Transaction {
            date: date.and_time(chrono::NaiveTime::MIN),
            state,
            code: code.map(Into::into),
            description,
            note: note.map(|n| n.1.comment),
            postings,
        })
    }
}

impl Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.date.format("%Y-%m-%d ").write_to(f)?;

        write!(f, "{}", self.state)?;

        if let Some(ref code) = self.code {
            write!(f, " ({code})")?;
        }

        write!(f, "{}", self.description)?;

        if let Some(ref note) = self.note {
            write!(f, "  ; {note}")?;
        }

        f.write_char('\n')?;

        for posting in self.postings.iter() {
            posting.fmt(f)?;
        }

        Ok(())
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum ValueExpr {
    Value { decimal: Decimal, commodity: String },
    String(String),
    Function { name: String, args: Vec<ValueExpr> },
}

impl Display for ValueExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueExpr::Value { decimal, commodity } => {
                write!(f, "{commodity} {decimal}")
            }
            ValueExpr::String(s) => write!(f, "\"{s}\""),
            ValueExpr::Function { name, args } => {
                write!(f, "{name}(")?;
                let mut args_iter = args.iter();
                if let Some(arg) = args_iter.next() {
                    write!(f, "{arg}")?;
                }
                for arg in args_iter {
                    write!(f, ", {arg}")?;
                }
                write!(f, ")")
            }
        }
    }
}

impl ValueExpr {
    pub fn parse_value(input: &mut &str) -> ModalResult<ValueExpr> {
        alt((
            (
                take_while(1.., |c| AsChar::is_alpha(c) || c == '$'),
                opt(one_of(b" ")),
                decimal,
            )
                .map(|r| ValueExpr::Value {
                    decimal: r.2,
                    commodity: r.0.to_string(),
                }),
            (
                decimal,
                opt(one_of(b" ")),
                take_while(1.., |c| AsChar::is_alpha(c) || c == '$'),
            )
                .map(|r| ValueExpr::Value {
                    decimal: r.0,
                    commodity: r.2.to_string(),
                }),
        ))
        .parse_next(input)
    }

    pub fn parse_string(input: &mut &str) -> ModalResult<ValueExpr> {
        ('"', take_till(0.., ['"', '\n']), '"')
            .map(|r: (_, &str, _)| ValueExpr::String(r.1.into()))
            .parse_next(input)
    }

    pub fn parse_function(input: &mut &str) -> ModalResult<ValueExpr> {
        (alpha1, '(', repeat(0.., Self::parse), ')')
            .map(|r| ValueExpr::Function {
                name: r.0.into(),
                args: r.2,
            })
            .parse_next(input)
    }

    pub fn parse_operator(input: &mut &str) -> ModalResult<ValueExpr> {
        use winnow::combinator::{Infix::{self, *}, Prefix, Postfix};
        winnow::combinator::expression(alt((Self::parse_function, Self::parse_value)))
            .prefix(dispatch! {any;
                               '-' => Prefix(12, |_, e: ValueExpr| Ok(ValueExpr::Function {
                                   name: "negate".into(),
                                   args: vec![e],
                               })),
                _ => fail,
            })
            .infix(dispatch! {any;
                              '+' => Left(5, |_, a, b| Ok(ValueExpr::Function {
                                  name: "+".into(),
                                  args: vec![a, b],
                              })),
                              '-' => Left(5, |_, a, b| Ok(ValueExpr::Function {
                                  name: "-".into(),
                                  args: vec![a, b],
                              })),
                              '*' => Left(7, |_, a, b| Ok(ValueExpr::Function {
                                  name: "*".into(),
                                  args: vec![a, b],
                              })),
                              '/' => Left(7, |_, a, b| Ok(ValueExpr::Function {
                                  name: "/".into(),
                                  args: vec![a, b],
                              })),
                              _ => fail,
            })
            .parse_next(input)
    }

    pub fn parse(input: &mut &str) -> ModalResult<ValueExpr> {
        alt((
            Self::parse_operator,
            Self::parse_value,
            Self::parse_string,
            Self::parse_function,
        ))
        .parse_next(input)
    }
}

#[cfg(test)]
mod test {
    use rust_decimal::dec;

    #[test]
    fn test_parse_value() {
        let mut input = "$12";
        let res = super::ValueExpr::parse_value(&mut input);
        assert_eq!(
            res,
            Ok(super::ValueExpr::Value {
                decimal: dec!(12.00),
                commodity: "$".into()
            })
        )
    }

    #[test]
    fn test_parse_negation() {
        let mut input = "-$12";
        let res = super::ValueExpr::parse_operator(&mut input);
        assert_eq!(
            res,
            Ok(super::ValueExpr::Function {
                name: "negate".into(),
                args: vec![
                    super::ValueExpr::Value {
                        decimal: dec!(12.00),
                        commodity: "$".into()
                    }
                ]
            })
        )
    }

    #[test]
    fn test_parse_operator() {
        let mut input = "$12 + $14";
        let res = super::ValueExpr::parse_operator(&mut input);
        assert_eq!(
            res,
            Ok(super::ValueExpr::Function {
                name: "+".into(),
                args: vec![
                    super::ValueExpr::Value {
                        decimal: dec!(12.00),
                        commodity: "$".into()
                    },
                    super::ValueExpr::Value {
                        decimal: dec!(14.00),
                        commodity: "$".into()
                    }
                ]
            })
        )
    }

    #[test]
    fn test_parse_operator_complex() {
        let mut input = "scrub(account(\"Liabilities:Mortgage:Densmore\")) + $12";
        let res = super::ValueExpr::parse_operator(&mut input);
        assert_eq!(
            res,
            Ok(super::ValueExpr::Function {
                name: "+".into(),
                args: vec![
                    super::ValueExpr::Value {
                        decimal: dec!(12.00),
                        commodity: "$".into()
                    },
                    super::ValueExpr::Value {
                        decimal: dec!(14.00),
                        commodity: "$".into()
                    }
                ]
            })
        )
    }
}

#[derive(Clone, Debug)]
pub struct Amount {
    pub value: ValueExpr,
    pub absolute: bool,
}

impl Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = &self.value;
        if self.absolute {
            write!(f, "={value}")
        } else {
            write!(f, "{value}")
        }
    }
}

fn decimal(input: &mut &str) -> ModalResult<Decimal> {
    take_while(1.., b"-.,0123456789")
        .verify_map(|r: &str| {
            Decimal::from_str_exact(r.chars().filter(|c| *c != ',').collect::<String>().as_str())
                .ok()
        })
        .parse_next(input)
}

impl Amount {
    pub fn parse(input: &mut &str) -> ModalResult<Amount> {
        (
            opt('='),
            alt((ValueExpr::parse, ('(', ValueExpr::parse, ')').map(|r| r.1))),
        )
            .map(|r| Amount {
                value: r.1,
                absolute: r.0.is_some(),
            })
            .parse_next(input)
    }
}

#[derive(Clone, Debug)]
pub enum Posting {
    Posting {
        account: String,
        amount: Option<Amount>,
        note: Option<CommentBody>,
        state: TransactionState,
    },
    Comment(CommentBody),
}

impl Posting {
    pub fn parse(input: &mut &str) -> ModalResult<Posting> {
        space1(input)?;

        fn posting_helper(input: &mut &str) -> ModalResult<Posting> {
            // State is optionally '*' or '!'
            let state = TransactionState::parse(input)?;

            let mut account = String::new();
            loop {
                match peek(alt((hard_stop, newline.value(())))).parse_next(input) {
                    Err(winnow::error::ErrMode::Backtrack(_)) => {
                        let c = take(1usize).parse_next(input)?;
                        account.push_str(c);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                    Ok(_) => break,
                };
            }

            let amount = opt((hard_stop, Amount::parse).map(|a| a.1)).parse_next(input)?;

            let _pricing = opt((space1, literal("@@"), space1, Amount::parse)).parse_next(input)?;

            let note = opt((hard_stop, Comment::parse)).parse_next(input)?;

            if note.is_none() {
                newline(input)?;
            }
            Ok(Posting::Posting {
                account,
                amount,
                state,
                note: note.map(|n| n.1.comment),
            })
        }

        alt((
            // TODO parse comments into literals etc
            Comment::parse.map(|c| Posting::Comment(c.comment)),
            posting_helper,
        ))
        .parse_next(input)
    }
}

impl Display for Posting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Posting::Comment(comment) => writeln!(f, "    ; {comment}"),
            Posting::Posting {
                account,
                amount,
                note,
                state,
            } => {
                write!(f, "    {state}{account}")?;
                if let Some(amount) = amount {
                    write!(f, "  {amount}")?;
                }

                if let Some(note) = note {
                    writeln!(f, "  ; {note}")
                } else {
                    writeln!(f)
                }
            }
        }
    }
}
