use std::fmt::{Display, Write};

use winnow::{
    IResult, Parser,
    ascii::{alphanumeric1, newline, space0, space1},
    combinator::{alt, repeat, separated_pair},
    token::{one_of, tag, take_till0, take_till1},
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
        let (input, res) = take_till0(|c| c == '\n')
            .try_map(|s: &str| Ok::<String, ()>(s.into()))
            .parse_next(input)?;
        Ok((input, res))
    }

    fn value(input: &str) -> IResult<&str, (&str, &str)> {
        separated_pair(
            take_till1(|c| " \t\n\r:".contains(c)),
            (tag(":"), (space1)),
            take_till0(|c| c == '\n'),
        )
        .parse_next(input)
    }

    fn tags(input: &str) -> IResult<&str, Vec<String>> {
        let input = tag(":").parse_next(input)?.0;

        repeat(
            1..,
            (alphanumeric1::<&str, _>, (tag(":"))).try_map(|(r, _)| Ok::<String, ()>(r.into())),
        )
        .parse_next(input)
    }

    fn comment_body(input: &str) -> IResult<&str, CommentBody> {
        let (input, _) = space0(input)?;
        let (input, res) = alt((
            Self::value
                .try_map(|c| Ok::<CommentBody, ()>(CommentBody::Value(c.0.into(), c.1.into()))),
            Self::tags.try_map(|c| Ok::<CommentBody, ()>(CommentBody::Tags(c))),
            Self::comment.try_map(|c| Ok::<CommentBody, ()>(CommentBody::Comment(c))),
        ))
        .parse_next(input)?;
        let (input, _) = newline.parse_next(input)?;
        Ok((input, res))
    }

    pub fn parse(input: &str) -> IResult<&str, Comment> {
        let (input, o) = one_of(";#|*").parse_next(input)?;
        let kind = match o {
            ';' => CommentKind::Semicolon,
            '#' => CommentKind::Hash,
            '|' => CommentKind::Pipe,
            '*' => CommentKind::Asterisk,
            _ => unreachable!(),
        };
        let (input, _) = space0(input)?;
        Self::comment_body
            .try_map(|comment| Ok::<Comment, ()>(Comment { kind, comment }))
            .parse_next(input)
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
