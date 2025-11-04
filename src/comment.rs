use std::fmt::{Display, Write};

use nom::{
    IResult, Parser,
    bytes::{complete::take_till1, tag, take_till},
    character::complete::{alphanumeric1, newline, one_of, space0, space1},
    combinator::map_res,
    multi::many1,
    sequence::separated_pair,
};

#[derive(Clone, Copy, Debug)]
pub enum CommentKind {
    Semicolon,
    Hash,
    Pipe,
    Asterisk,
}

#[derive(Clone, Debug)]
pub enum CommentBody {
    Comment(String),
    Tags(Vec<String>),
    Value(String, String),
}

impl Display for CommentBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentBody::Comment(c) => c.fmt(f),
            CommentBody::Tags(tags) => {
                write!(f, ":{}:", tags.join(":"))
            }
            CommentBody::Value(key, value) => {
                write!(f, "{key}: {value}")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Comment {
    pub kind: CommentKind,
    pub comment: CommentBody,
}

impl Comment {
    fn comment(input: &str) -> IResult<&str, String> {
        let (input, res) = map_res(take_till(|c| c == '\n'), |s: &str| {
            Ok::<String, ()>(s.into())
        })
        .parse(input)?;
        Ok((input, res))
    }

    fn value(input: &str) -> IResult<&str, (&str, &str)> {
        separated_pair(
            take_till1(|c| " \t\n\r:".contains(c)),
            tag(":").and(space1),
            take_till(|c| c == '\n'),
        )
        .parse(input)
    }

    fn tags(input: &str) -> IResult<&str, Vec<String>> {
        let input = tag(":").parse(input)?.0;

        many1(map_res(alphanumeric1::<&str, _>.and(tag(":")), |(r, _)| {
            Ok::<String, ()>(r.into())
        }))
        .parse(input)
    }

    fn comment_body(input: &str) -> IResult<&str, CommentBody> {
        let (input, _) = space0(input)?;
        let (input, res) = nom::branch::alt((
            map_res(Self::value, |c| {
                Ok::<CommentBody, ()>(CommentBody::Value(c.0.into(), c.1.into()))
            }),
            map_res(Self::tags, |c| Ok::<CommentBody, ()>(CommentBody::Tags(c))),
            map_res(Self::comment, |c| {
                Ok::<CommentBody, ()>(CommentBody::Comment(c))
            }),
        ))
        .parse(input)?;
        let (input, _) = newline.parse(input)?;
        Ok((input, res))
    }

    pub fn parse(input: &str) -> IResult<&str, Comment> {
        let (input, o) = one_of(";#|*")(input)?;
        let kind = match o {
            ';' => CommentKind::Semicolon,
            '#' => CommentKind::Hash,
            '|' => CommentKind::Pipe,
            '*' => CommentKind::Asterisk,
            _ => unreachable!(),
        };
        let (input, _) = space0(input)?;
        map_res(Self::comment_body, |comment| {
            Ok::<Comment, ()>(Comment { kind, comment })
        })
        .parse(input)
    }
}

impl Display for Comment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_char(match self.kind {
            CommentKind::Semicolon => ';',
            CommentKind::Hash => '#',
            CommentKind::Pipe => '|',
            CommentKind::Asterisk => '*',
        })?;

        writeln!(f, " {}", self.comment)
    }
}
