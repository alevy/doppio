use std::{collections::HashMap, fmt::Display};

use winnow::ModalResult;
use winnow::ascii::alphanumeric1;
use winnow::combinator::alt;
use winnow::token::{literal, one_of};
use winnow::{
    Parser,
    ascii::{newline, space0, space1},
    combinator::{opt, repeat},
    token::take_till,
};

#[derive(Clone, Debug)]
pub enum Command {
    Alias {
        alias: String,
        origin: String,
    },
    Include(String),
    Price(String),
    Define(String),
    Account {
        name: String,
        sub_directives: HashMap<String, Option<String>>,
    },
    Payee(String),
    Commodity {
        name: String,
        sub_directives: HashMap<String, Option<String>>,
    },
    Tag {
        name: String,
        sub_directives: HashMap<String, Option<String>>,
    },
}

impl Command {
    fn alias(input: &mut &str) -> ModalResult<Command> {
        literal("alias").parse_next(input)?;
        space0(input)?;

        let alias = take_till(0.., |c| c == '=').parse_next(input)?;
        space0(input)?;
        literal("=").parse_next(input)?;
        space0(input)?;
        let origin = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Alias {
            alias: alias.into(),
            origin: origin.into(),
        })
    }

    fn include(input: &mut &str) -> ModalResult<Command> {
        literal("include").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Include(res.into()))
    }

    fn price(input: &mut &str) -> ModalResult<Command> {
        literal("P").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Price(res.into()))
    }

    fn define(input: &mut &str) -> ModalResult<Command> {
        literal("define").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Define(res.into()))
    }

    fn sub_directive(input: &mut &str) -> ModalResult<(String, Option<String>)> {
        space1(input)?;
        let key: &str = alt((alphanumeric1,)).parse_next(input)?;
        let value =
            opt((space1, take_till(0.., |c| c == '\n')).map(|(_, v): (_, &str)| v.to_string()))
                .parse_next(input)?;
        newline.parse_next(input)?;

        Ok((key.to_string(), value))
    }

    fn account(input: &mut &str) -> ModalResult<Command> {
        literal("account").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;

        let mut sub_directives: Vec<(String, Option<String>)> =
            repeat(.., Self::sub_directive).parse_next(input)?;

        Ok(Command::Account {
            name: res.into(),
            sub_directives: sub_directives.drain(..).collect(),
        })
    }

    fn payee(input: &mut &str) -> ModalResult<Command> {
        literal("payee").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Payee(res.into()))
    }

    fn commodity(input: &mut &str) -> ModalResult<Command> {
        literal("commodity").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;

        let mut sub_directives: Vec<(String, Option<String>)> =
            repeat(.., Self::sub_directive).parse_next(input)?;

        Ok(Command::Commodity {
            name: res.into(),
            sub_directives: sub_directives.drain(..).collect(),
        })
    }

    fn tag(input: &mut &str) -> ModalResult<Command> {
        literal("tag").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;

        let mut sub_directives: Vec<(String, Option<String>)> =
            repeat(.., Self::sub_directive).parse_next(input)?;

        Ok(Command::Tag {
            name: res.into(),
            sub_directives: sub_directives.drain(..).collect(),
        })
    }

    pub fn parse(input: &mut &str) -> ModalResult<Command> {
        // Preceding command lines with ! or @ is deprecated
        opt(one_of(b"!@")).parse_next(input)?;
        alt((
            Self::alias,
            Self::include,
            Self::price,
            Self::define,
            Self::account,
            Self::payee,
            Self::commodity,
            Self::tag,
        ))
        .parse_next(input)
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Alias { alias, origin } => writeln!(f, "alias {alias} = {origin}"),
            Command::Include(path) => writeln!(f, "include {path}"),
            Command::Price(p) => writeln!(f, "P {p}"),
            Command::Define(d) => writeln!(f, "define {d}"),
            Command::Account {
                name,
                sub_directives,
            } => {
                writeln!(f, "account {name}")?;
                for (key, value) in sub_directives {
                    write!(f, "  {key}")?;
                    if let Some(value) = value {
                        write!(f, " {value}")?;
                    }
                    writeln!(f)?;
                }
                Ok(())
            }
            Command::Payee(p) => writeln!(f, "payee {p}"),
            Command::Commodity {
                name,
                sub_directives,
            } => {
                writeln!(f, "commodity {name}")?;
                for (key, value) in sub_directives {
                    write!(f, "  {key}")?;
                    if let Some(value) = value {
                        write!(f, " {value}")?;
                    }
                    writeln!(f)?;
                }
                Ok(())
            }
            Command::Tag {
                name,
                sub_directives,
            } => {
                writeln!(f, "tag {name}")?;
                for (key, value) in sub_directives {
                    write!(f, "  {key}")?;
                    if let Some(value) = value {
                        write!(f, " {value}")?;
                    }
                    writeln!(f)?;
                }
                Ok(())
            }
        }
    }
}
