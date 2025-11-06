use std::{
    collections::HashMap,
    fmt::{Display, Write},
};

use winnow::{
    PResult, Parser,
    ascii::{newline, space0, space1},
    combinator::{alt, delimited, opt, peek, repeat},
    error::ContextError,
    token::{literal, one_of, take, take_till},
};

use crate::{
    Comment, CommentBody,
    helpers::{amount, hard_stop},
};

#[derive(Clone, Debug)]
pub enum TransactionState {
    Cleared,
    Pending,
    Uncleared,
}

impl TransactionState {
    pub fn parse(input: &mut &str) -> PResult<TransactionState> {
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
    pub date: chrono::NaiveDate,
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

    pub fn parse(input: &mut &str) -> PResult<Transaction> {
        // Date as Y-M-D or Y/M/D
        let date = chrono::NaiveDate::parse_and_remainder(input, "%Y-%m-%d")
            .or_else(|_| chrono::NaiveDate::parse_and_remainder(input, "%Y/%m/%d"))
            .map_or_else(
                |_| Err(winnow::error::ErrMode::Backtrack(ContextError::new())),
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
                Err(e) => return Err(e),
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
            date,
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
        self.date.format("%Y-%m-%d").write_to(f)?;

        write!(f, "{}", self.state)?;

        if let Some(ref code) = self.code {
            write!(f, "({code}) ")?;
        }

        f.write_str(&self.description)?;

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

#[derive(Clone, Debug)]
pub enum Posting {
    Posting {
        account: String,
        amount: Option<String>,
        note: Option<CommentBody>,
        state: TransactionState,
    },
    Comment(CommentBody),
}

impl Posting {
    pub fn parse(input: &mut &str) -> PResult<Posting> {
        space1(input)?;

        fn posting_helper(input: &mut &str) -> PResult<Posting> {
            // State is optionally '*' or '!'
            let state = TransactionState::parse(input)?;

            let mut account = String::new();
            loop {
                match peek(alt((hard_stop, newline.value(())))).parse_next(input) {
                    Err(winnow::error::ErrMode::Backtrack(_)) => {
                        let c = take(1usize).parse_next(input)?;
                        account.push_str(c);
                    }
                    Err(e) => return Err(e),
                    Ok(_) => break,
                };
            }

            let amount = opt((hard_stop, amount).map(|a| a.1)).parse_next(input)?;

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
