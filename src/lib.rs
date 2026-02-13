use std::{
    collections::BTreeMap, fmt::Display, fs::File, io::Read, path::PathBuf
};

use winnow::{
    ModalResult, Parser,
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
    pub fn parse(input: &mut &str) -> ModalResult<JournalNode> {
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
pub struct JournalAst(pub Vec<JournalNode>);

fn resolve_paths(base: &PathBuf, pattern: &PathBuf) -> Vec<PathBuf> {
    let base = base.parent().map(Into::<PathBuf>::into).unwrap_or("".into());
    let pattern = pattern.components().fold(base.clone(), |pb, c| pb.join(c));
    let files = glob::glob(pattern.to_str().unwrap_or("")).unwrap();
    files.filter_map(Result::ok).collect()
}

impl JournalAst {
    pub fn parse(input: &mut &str) -> ModalResult<JournalAst> {
        repeat_till(0.., JournalNode::parse, eof)
            .map(|(nodes, _): (Vec<JournalNode>, &str)| JournalAst(nodes.into_iter().collect()))
            .parse_next(input)
    }

    pub fn resolve_includes<P: Into<PathBuf>>(
        &mut self,
        original_file: P,
    ) -> ModalResult<()> {
        let original_file = original_file.into();
        while let Some((i, path)) = self
            .0
            .iter()
            .enumerate()
            .filter_map(|(i, node)| {
                if let JournalNode::Command(Command::Include(path)) = node {
                    Some((i, path.clone()))
                } else {
                    None
                }
            })
            .next()
        {
            let mut module = String::new();
            let mypaths = resolve_paths(&original_file, &path.into());
            let mut nodes = vec![];
            for mypath in mypaths.iter() {
                File::open(&mypath)
                    .expect(&mypath.to_str().unwrap())
                    .read_to_string(&mut module)
                    .unwrap();
                let mut module = module.as_str();
                let mut new_journal = JournalAst::parse(&mut module)?;
                new_journal.resolve_includes(mypath)?;
                nodes.extend(new_journal.0);
            }
            self.0.splice(i..=i, nodes);
        }
        Ok(())
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

impl Display for JournalAst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for node in self.0.iter() {
            node.fmt(f)?;
        }
        Ok(())
    }
}

pub struct Account {
    pub name: String,
    pub note: Option<String>,
}

pub struct Journal {
    pub accounts: BTreeMap<String, Account>,
}

impl Journal {
    pub fn compile(ast: &JournalAst) -> Result<Self, ()> {
        let mut result = Journal {
            accounts: Default::default(),
        };
        for node in ast.0.iter() {
            match node {
                JournalNode::Command(command) => {
                    match command {
                        Command::Account { name, sub_directives } => {
                            let account = Account {
                                name: name.clone(),
                                note: sub_directives.get("note").cloned().flatten(),
                            };
                            result.accounts.insert(name.clone(), account);
                        },
                        // Command::Alias { alias, origin } => todo!(),
                        // Command::Include(_) => todo!(),
                        // Command::Price(_) => todo!(),
                        // Command::Define(_) => todo!(),
                        // Command::Payee(_) => todo!(),
                        // Command::Commodity { name, sub_directives } => todo!(),
                        // Command::Tag { name, sub_directives } => todo!(),
                        _ => {},
                    }
                },
                JournalNode::Comment(_comment) => {},
                JournalNode::Transaction(_transaction) => {},
            }
        }
        Ok(result)
    }
}
