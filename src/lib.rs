use std::fmt::{Display, Write};

use nom::{
    IResult, Parser,
    combinator::{eof, map_res},
    multi::many_till,
};

pub mod comment;
pub use comment::*;

pub mod command;
pub use command::*;

pub mod transaction;
pub use transaction::*;

mod helpers;

#[derive(Clone, Debug)]
pub enum JournalNode {
    Comment(Comment),
    Command(Command),
    Transaction(Transaction),
    EmptyLine,
}

impl Display for JournalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Comment(comment) => comment.fmt(f),
            Self::Command(command) => command.fmt(f),
            Self::Transaction(transaction) => transaction.fmt(f),
            Self::EmptyLine => f.write_char('\n'),
        }
    }
}

impl JournalNode {
    pub fn parse(input: &str) -> IResult<&str, JournalNode> {
        nom::branch::alt((
            map_res(Comment::parse, |c| {
                Ok::<JournalNode, ()>(JournalNode::Comment(c))
            }),
            map_res(Transaction::parse, |c| {
                Ok::<JournalNode, ()>(JournalNode::Transaction(c))
            }),
            map_res(Command::parse, |c| {
                Ok::<JournalNode, ()>(JournalNode::Command(c))
            }),
            map_res(helpers::empty_line, |_| {
                Ok::<JournalNode, ()>(JournalNode::EmptyLine)
            }),
        ))
        .parse(input)
    }
}

#[derive(Debug)]
pub struct Journal(pub Vec<JournalNode>);

impl Journal {
    pub fn parse(input: &str) -> IResult<&str, Journal> {
        map_res(many_till(JournalNode::parse, eof), |(nodes, _)| {
            Ok::<Journal, ()>(Journal(nodes.into_iter().collect()))
        })
        .parse(input)
    }
}

impl Display for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for node in self.0.iter() {
            node.fmt(f)?;
        }
        Ok(())
    }
}
