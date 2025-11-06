use std::fmt::{Display, Write};

use winnow::{
    ascii::{alphanumeric1, newline, space0, space1}, combinator::{alt, repeat, separated_pair}, token::{one_of, tag, take_till, take_till0, take_till1}, IResult, PResult, Parser
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
    fn comment(input: &mut &str) -> PResult<String> {
        let res = take_till(0.., |c| c == '\n')
            .map(|s: &str| s.into())
            .parse_next(input)?;
        Ok(res)
    }

    fn value(input: &mut &str) -> PResult<(String, String)> {
        separated_pair(
            take_till1(|c| " \t\n\r:".contains(c)).map(Into::into),
            (tag(":"), (space1)),
            take_till0(|c| c == '\n').map(Into::into),
        )
        .parse_next(input)
    }

    fn tags(input: &mut &str) -> PResult<Vec<String>> {
        tag(":").parse_next(input)?;

        repeat(
            1..,
            (alphanumeric1.map(String::from), tag(":")).map(|r| r.0),
        )
        .parse_next(input)
    }

    fn comment_body(input: &mut &str) -> PResult<CommentBody> {
        space0(input)?;
        let res = alt((
            Self::value
                .map(|c| CommentBody::Value(c.0.into(), c.1.into())),
            Self::tags.map(|c| CommentBody::Tags(c)),
            Self::comment.map(|c| CommentBody::Comment(c)),
        ))
        .parse_next(input)?;
        newline.parse_next(input)?;
        Ok(res)
    }

    pub fn parse(input: &mut &str) -> PResult<Comment> {
        let o = one_of(b";#|*").parse_next(input)?;
        let kind = match o {
            ';' => CommentKind::Semicolon,
            '#' => CommentKind::Hash,
            '|' => CommentKind::Pipe,
            '*' => CommentKind::Asterisk,
            _ => unreachable!(),
        };
        space0(input)?;
        Self::comment_body
            .map(|comment| Comment { kind, comment })
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
