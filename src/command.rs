use std::fmt::Display;

use nom::{
    IResult, Parser,
    bytes::{tag, take_till},
    character::complete::{newline, one_of, space0},
    combinator::opt,
};

#[derive(Clone, Debug)]
pub enum Command {
    Include(String),
    Price(String),
    Account(String),
    Payee(String),
    Commodity(String),
    Tag(String),
}

impl Command {
    fn include(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("include").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Include(res.into())))
    }

    fn price(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("P").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Price(res.into())))
    }

    fn account(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("account").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Account(res.into())))
    }

    fn payee(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("payee").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Payee(res.into())))
    }

    fn commodity(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("commodity").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Commodity(res.into())))
    }

    fn tag(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("tag").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Tag(res.into())))
    }

    pub fn parse(input: &str) -> IResult<&str, Command> {
        // Preceding command lines with ! or @ is deprecated
        let (input, _) = opt(one_of("!@")).parse(input)?;
        nom::branch::alt((
            Self::include,
            Self::price,
            Self::account,
            Self::payee,
            Self::commodity,
            Self::tag,
        ))
        .parse(input)
    }
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Include(path) => writeln!(f, "include {path}"),
            Command::Price(p) => writeln!(f, "P {p}"),
            Command::Account(a) => writeln!(f, "account {a}"),
            Command::Payee(p) => writeln!(f, "payee {p}"),
            Command::Commodity(c) => writeln!(f, "commodity {c}"),
            Command::Tag(t) => writeln!(f, "tag {t}"),
        }
    }
}
