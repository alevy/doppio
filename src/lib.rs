use std::fmt::Display;

use winnow::{
    PResult, Parser,
    combinator::{alt, eof, repeat, repeat_till},
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
    pub fn parse(input: &mut &str) -> PResult<JournalNode> {
        let () = repeat(.., helpers::empty_line).parse_next(input)?;
        alt((
            Comment::parse.map(JournalNode::Comment),
            Transaction::parse.map(JournalNode::Transaction),
            Command::parse.map(JournalNode::Command),
        ))
        .parse_next(input)
    }
}

#[derive(Debug)]
pub struct Journal(pub Vec<JournalNode>);

impl Journal {
    pub fn parse(input: &mut &str) -> PResult<Journal> {
        repeat_till(0.., JournalNode::parse, eof)
            .map(|(nodes, _): (Vec<JournalNode>, &str)| Journal(nodes.into_iter().collect()))
            .parse_next(input)
    }

    pub fn add_txn(&mut self, txn: Transaction) {
        let i = self
            .0
            .iter()
            .enumerate()
            .find_map(|(i, jn)| {
                if let JournalNode::Transaction(ti) = jn
                    && ti.date > txn.date
                {
                    return Some(i);
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
