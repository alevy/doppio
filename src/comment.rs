use std::fmt::{Display, Write};

use nom::{
    IResult, Parser,
    bytes::take_till,
    character::complete::{newline, one_of, space0},
    combinator::map_res,
};

#[derive(Clone, Copy, Debug)]
pub enum CommentKind {
    Semicolon,
    Hash,
    Pipe,
    Asterisk,
}

#[derive(Clone, Debug)]
pub struct Comment {
    pub kind: CommentKind,
    pub comment: String,
}

impl Comment {
    fn comment_body(input: &str) -> IResult<&str, String> {
        let (input, _) = space0(input)?;
        let (input, res) = map_res(take_till(|c| c == '\n'), |s: &str| {
            Ok::<String, ()>(s.into())
        })
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
