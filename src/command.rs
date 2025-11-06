use std::{collections::HashMap, fmt::Display};

use winnow::{
    bytes::{tag, take_till0}, character::complete::{newline, one_of, space0, space1}, combinator::opt, multi::many0, IResult, Parser
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
    Tag(String),
}

impl Command {
    fn include(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("include").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Include(res.into())))
    }

    fn price(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("P").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Price(res.into())))
    }

    fn account(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("account").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;

        let (input, mut sub_directives): (&str, Vec<(String, String)>) = many0(|input| {
            let (input, _) = space1(input)?;
            let (input, key): (&str, &str) = winnow::branch::alt((
                tag("note"),
            )).parse(input)?;
            let (input, _) = space1(input)?;
            let (input, value) = take_till0(|c| c == '\n').parse(input)?;

            Ok((input, (key.to_string(), value.to_string())))
        }).parse(input)?;


        Ok((input, Command::Account {
            name: res.into(),
            sub_directives: sub_directives.drain(..).collect(),
        }))
    }

    fn payee(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("payee").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Payee(res.into())))
    }

    fn commodity(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("commodity").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Commodity(res.into())))
    }

    fn tag(input: &str) -> IResult<&str, Command> {
        let (input, _) = tag("tag").parse(input)?;
        let (input, _) = space0(input)?;

        let (input, res) = take_till0(|c| c == '\n').parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, Command::Tag(res.into())))
    }

    pub fn parse(input: &str) -> IResult<&str, Command> {
        // Preceding command lines with ! or @ is deprecated
        let (input, _) = opt(one_of("!@")).parse(input)?;
        winnow::branch::alt((
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
            Command::Account { name, sub_directives } => {
                writeln!(f, "account {name}")?;
                for (key, value) in sub_directives {
                    writeln!(f, "{key} {value}")?;
                }
                Ok(())
            },
            Command::Payee(p) => writeln!(f, "payee {p}"),
            Command::Commodity(c) => writeln!(f, "commodity {c}"),
            Command::Tag(t) => writeln!(f, "tag {t}"),
        }
    }
}
