use std::{collections::HashMap, fmt::{Display, Write}};

use nom::{
    IResult, Parser,
    bytes::{tag, take, take_till1},
    character::{
        complete::{newline, one_of, space0, space1},
        digit1,
    },
    combinator::{map_res, opt, peek, value},
    multi::many0,
    sequence::delimited,
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
    pub fn parse(input: &str) -> IResult<&str, TransactionState> {
        // State is optionally '*' or '!'
        let (input, state) = map_res(opt(one_of("*!")), |c| {
            Ok::<TransactionState, ()>(match c {
                Some('*') => TransactionState::Cleared,
                Some('!') => TransactionState::Pending,
                _ => TransactionState::Uncleared,
            })
        })
        .parse(input)?;
        let input = space0(input)?.0;
        Ok((input, state))
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
    pub year: i32,
    pub month: u32,
    pub date: u32,
    pub state: TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub note: Option<CommentBody>,
    pub postings: Vec<Posting>,
}

impl Transaction {
    pub fn tags(&self) -> Vec<String> {
        self.postings.iter().filter_map(|posting| {
            if let Posting::Comment(CommentBody::Tags(tags)) = posting {
                Some(tags)
            } else {
                None
            }
        }).fold(vec![], |mut f, s| {
            f.append(&mut s.clone());
            f
        })
    }

    pub fn value_of<S: AsRef<str>>(&self, key: S) -> Option<&String> {
        self.postings.iter().find_map(|posting| {
            if let Posting::Comment(CommentBody::Value(k, v)) = posting && k == key.as_ref() {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn metadata(&self) -> HashMap<&String, &String> {
        self.postings.iter().filter_map(|posting| {
            if let Posting::Comment(CommentBody::Value(k, v)) = posting {
                Some((k, v))
            } else {
                None
            }
        }).collect()
    }

    pub fn parse(input: &str) -> IResult<&str, Transaction> {
        // Date as Y-M-D or Y/M/D
        let (input, year) = map_res(digit1(), str::parse).parse(input)?;
        let (input, _) = one_of("/-")(input)?;
        let (input, month) = map_res(digit1(), str::parse).parse(input)?;
        let (input, _) = one_of("/-")(input)?;
        let (input, date) = map_res(digit1(), str::parse).parse(input)?;

        let (input, _) = space0(input)?;

        // State is optionally '*' or '!'
        let (input, state) = TransactionState::parse(input)?;

        let (input, code) =
            opt(delimited(tag("("), take_till1(|c| c == ')'), tag(")"))).parse(input)?;

        let input = space0(input)?.0;

        // description
        let mut input = input;
        let mut description = String::new();
        input = loop {
            input = match peek((value((), hard_stop.and(tag(";")))).or(value((), newline)))
                .parse(input)
            {
                Err(nom::Err::Error(input)) => {
                    let (input, c) = take(1usize).parse(input.input)?;
                    description.push_str(c);
                    input
                }
                Err(e) => return Err(e),
                Ok((input, _)) => break input,
            };
        };

        // note
        let (mut input, note) = opt(hard_stop.and(Comment::parse)).parse(input)?;
        if note.is_none() {
            (input, _) = newline(input)?;
        }

        let (input, postings) = many0(Posting::parse).parse(input)?;

        Ok((
            input,
            Transaction {
                year,
                month,
                date,
                state,
                code: code.map(Into::into),
                description,
                note: note.map(|n| n.1.comment),
                postings,
            },
        ))
    }
}

impl Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{:02}-{:02} ", self.year, self.month, self.date)?;

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
    pub fn parse(input: &str) -> IResult<&str, Posting> {
        let (input, _) = space1(input)?;

        fn posting_helper(mut input: &str) -> IResult<&str, Posting> {
            // State is optionally '*' or '!'
            let state;
            (input, state) = TransactionState::parse(input)?;

            let mut account = String::new();
            input = loop {
                input = match peek(hard_stop.or(value((), newline))).parse(input) {
                    Err(nom::Err::Error(input)) => {
                        let (input, c) = take(1usize).parse(input.input)?;
                        account.push_str(c);
                        input
                    }
                    Err(e) => return Err(e),
                    Ok((input, _)) => break input,
                };
            };

            let (input, amount) = opt(hard_stop.and(amount)).parse(input)?;
            let amount = amount.map(|a| a.1);

            let (mut input, note) = opt(hard_stop.and(Comment::parse)).parse(input)?;

            if note.is_none() {
                (input, _) = newline(input)?;
            }
            Ok((
                input,
                Posting::Posting {
                    account,
                    amount,
                    state,
                    note: note.map(|n| n.1.comment),
                },
            ))
        }

        nom::branch::alt((
            // TODO parse comments into tags etc
            map_res(Comment::parse, |c| {
                Ok::<Posting, ()>(Posting::Comment(c.comment))
            }),
            posting_helper,
        ))
        .parse(input)
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
                    writeln!(f, "")
                }
            }
        }
    }
}
