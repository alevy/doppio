use std::{collections::HashMap, fmt::Display};

use winnow::PResult;
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
    Include(String),
    Price(String),
    Account {
        name: String,
        sub_directives: HashMap<String, String>,
    },
    Payee(String),
    Commodity(String),
    Literal(String),
}

impl Command {
    fn include(input: &mut &str) -> PResult<Command> {
        literal("include").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Include(res.into()))
    }

    fn price(input: &mut &str) -> PResult<Command> {
        literal("P").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Price(res.into()))
    }

    fn sub_directive(input: &mut &str) -> PResult<(String, String)> {
        space1(input)?;
        let key: &str = alt((literal("note"),)).parse_next(input)?;
        space1(input)?;
        let value: &str = take_till(0.., |c| c == '\n').parse_next(input)?;

        Ok((key.to_string(), value.to_string()))
    }

    fn account(input: &mut &str) -> PResult<Command> {
        literal("account").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;

        let mut sub_directives: Vec<(String, String)> =
            repeat(.., Self::sub_directive).parse_next(input)?;

        Ok(Command::Account {
            name: res.into(),
            sub_directives: sub_directives.drain(..).collect(),
        })
    }

    fn payee(input: &mut &str) -> PResult<Command> {
        literal("payee").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Payee(res.into()))
    }

    fn commodity(input: &mut &str) -> PResult<Command> {
        literal("commodity").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Commodity(res.into()))
    }

    fn tag(input: &mut &str) -> PResult<Command> {
        literal("tag").parse_next(input)?;
        space0(input)?;

        let res = take_till(0.., |c| c == '\n').parse_next(input)?;
        newline.parse_next(input)?;
        Ok(Command::Literal(res.into()))
    }

    pub fn parse(input: &mut &str) -> PResult<Command> {
        // Preceding command lines with ! or @ is deprecated
        opt(one_of(b"!@")).parse_next(input)?;
        alt((
            Self::include,
            Self::price,
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
            Command::Include(path) => writeln!(f, "include {path}"),
            Command::Price(p) => writeln!(f, "P {p}"),
            Command::Account {
                name,
                sub_directives,
            } => {
                writeln!(f, "account {name}")?;
                for (key, value) in sub_directives {
                    writeln!(f, "{key} {value}")?;
                }
                Ok(())
            }
            Command::Payee(p) => writeln!(f, "payee {p}"),
            Command::Commodity(c) => writeln!(f, "commodity {c}"),
            Command::Literal(t) => writeln!(f, "tag {t}"),
        }
    }
}
