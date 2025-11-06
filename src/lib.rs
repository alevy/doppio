use std::fmt::Display;

use nom::{
    combinator::{eof, map_res}, multi::{many0, many_till}, IResult, Parser
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
}

impl Display for JournalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Comment(comment) => comment.fmt(f),
            Self::Command(command) => command.fmt(f),
            Self::Transaction(transaction) => transaction.fmt(f),
        }
    }
}

impl JournalNode {
    pub fn parse(input: &str) -> IResult<&str, JournalNode> {
        let (input, _) = many0(helpers::empty_line).parse(input)?;
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

    pub fn add_txn(&mut self, txn: Transaction) {
        let i = self
            .0
            .iter()
            .enumerate()
            .find_map(|(i, jn)| {
                if let JournalNode::Transaction(ti) = jn {
                    if ti.date > txn.date {
                        return Some(i);
                    }
                }
                None
            })
            .unwrap_or(self.0.len());

        self.0.insert(i, JournalNode::Transaction(txn));
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
