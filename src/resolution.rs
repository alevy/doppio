use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::ast;

#[derive(Debug)]
pub struct HIR {
    pub entries: Vec<ResolutionEntry>,
    pub contexts: Vec<Context>,
    pub global_context: GlobalContext,
}

impl Default for HIR {
    fn default() -> Self {
        Self {
            entries: vec![],
            contexts: vec![Context::default()],
            global_context: Default::default(),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct Context {
    pub account_aliases: BTreeMap<String, String>,
    pub commodity_aliases: BTreeMap<String, String>,
    pub default_commodity: Option<String>,
}

#[derive(Default, Debug)]
pub struct GlobalContext {
    pub commodity_properties: BTreeMap<String, CommodityProperties>,
}

#[derive(Default, Debug)]
pub struct CommodityProperties {
    pub format: Option<String>,
    pub no_market: bool,
    pub note: Option<String>,
}

#[derive(Debug)]
pub struct ResolutionEntry {
    pub context_id: usize, // index into `Journal#contexts`
    pub data: Entry,
}

#[derive(Debug)]
pub enum Entry {
    Transaction(Transaction),
    Price(()),
    Assertion(()),
}

#[derive(Debug)]
pub struct Transaction {
    pub date: NaiveDate,
    pub secondary_date: Option<NaiveDate>,
    pub state: ast::TransactionState,
    pub code: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub postings: Vec<Posting>,
}

#[derive(Debug)]
pub struct Posting {
    pub account: String,
    pub amount: Option<ast::AmountDetails>,
    pub state: ast::TransactionState,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum ResolutionError {
    InvalidDate,
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionError::InvalidDate => {
                write!(f, "Invalid date")
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

impl HIR {
    fn resolve_date(
        ast: &ast::Date,
        fallback_year: Option<u16>,
    ) -> Result<NaiveDate, ResolutionError> {
        let year = ast
            .year
            .or(fallback_year)
            .ok_or(ResolutionError::InvalidDate)?;
        NaiveDate::from_ymd_opt(year.into(), ast.month.into(), ast.date.into())
            .ok_or(ResolutionError::InvalidDate)
    }

    fn resolve_metadata(notes: Vec<String>) -> (Vec<String>, BTreeMap<String, String>) {
        let mut tags: Vec<String> = vec![];
        let mut metadata: BTreeMap<String, String> = Default::default();

        for note in notes {
            let note = note.trim();
            if let Some(note) = note.strip_prefix(":")
                && let Some(note) = note.strip_suffix(":")
            {
                for tag in note.split(":") {
                    tags.push(tag.into());
                }
            } else if let Some((key, value)) = note.split_once(":") {
                metadata.insert(key.trim().into(), value.trim().into());
            }
        }
        (tags, metadata)
    }
}

impl TryFrom<ast::Journal> for HIR {
    type Error = ResolutionError;

    fn try_from(ast: ast::Journal) -> Result<Self, Self::Error> {
        let mut result: HIR = Default::default();

        #[allow(unused_mut)]
        let mut current_default_year = None;

        for entry in ast.entries {
            let mut new_context: Option<Context> = None;
            let context_id = result.contexts.len() - 1; // always at least zero;
            let context = &result.contexts[context_id];
            match entry {
                ast::Entry::Directive(ast::Directive::Unknown(_)) | ast::Entry::Comment(_) => {
                    // Discard
                }
                ast::Entry::Directive(ast::Directive::Commodity {
                    name,
                    notes: _,
                    items,
                }) => {
                    let global_context = result
                        .global_context
                        .commodity_properties
                        .entry(name.clone())
                        .or_default();
                    for item in items {
                        match item {
                            ast::CommodityItem::Alias(alias) => {
                                new_context = Some(new_context.unwrap_or_else(|| context.clone()))
                                    .map(|mut ctx| {
                                        ctx.commodity_aliases.insert(alias, name.clone());
                                        ctx
                                    });
                            }
                            ast::CommodityItem::Default => {
                                new_context = Some(new_context.unwrap_or_else(|| context.clone()))
                                    .map(|mut ctx| {
                                        ctx.default_commodity = Some(name.clone());
                                        ctx
                                    });
                            }
                            ast::CommodityItem::Format(format) => {
                                global_context.format = Some(format);
                            }
                            ast::CommodityItem::NoMarket => {
                                global_context.no_market = true;
                            }
                            ast::CommodityItem::Note(note) => {
                                global_context.note = Some(note);
                            }
                            ast::CommodityItem::Unknown(key, value) => todo!("{key} {value:?}"),
                        }
                    }
                }
                ast::Entry::Directive(ast::Directive::Account(account)) => {
                    todo!("{account}")
                }
                ast::Entry::Transaction(transaction) => {
                    let date = Self::resolve_date(&transaction.date, current_default_year)?;
                    let secondary_date = if let Some(ref d) = transaction.secondary_date {
                        Some(Self::resolve_date(d, current_default_year)?)
                    } else {
                        None
                    };

                    let (tags, metadata) = Self::resolve_metadata(transaction.notes);
                    let postings = transaction
                        .postings
                        .into_iter()
                        .map(|p| {
                            let (tags, metadata) = Self::resolve_metadata(p.notes);

                            Posting {
                                account: p.account,
                                amount: p.amount,
                                state: p.state,
                                tags,
                                metadata,
                            }
                        })
                        .collect();

                    let data = Entry::Transaction(Transaction {
                        date,
                        secondary_date,
                        state: transaction.state,
                        code: transaction.code,
                        description: transaction.description,
                        tags,
                        metadata,
                        postings,
                    });

                    result.entries.push(ResolutionEntry { context_id, data });
                }
            }

            if let Some(new_context) = new_context {
                result.contexts.push(new_context);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod resolution_tests {
    use chrono::Datelike;

    use super::*;
    use crate::ast;

    #[test]
    fn test_date_resolution() {
        // Case: Successful full date
        let d1 = ast::Date {
            year: Some(2024),
            month: 2,
            date: 29,
        };
        assert!(HIR::resolve_date(&d1, None).is_ok());

        // Case: Fallback year logic
        let d2 = ast::Date {
            year: None,
            month: 1,
            date: 15,
        };
        let resolved = HIR::resolve_date(&d2, Some(2023)).unwrap();
        assert_eq!(resolved.year(), 2023);

        // Case: No year available (Error)
        assert!(matches!(
            HIR::resolve_date(&d2, None),
            Err(ResolutionError::InvalidDate)
        ));

        // Case: Calendar invalidity (Feb 30)
        let d3 = ast::Date {
            year: Some(2023),
            month: 2,
            date: 30,
        };
        assert!(matches!(
            HIR::resolve_date(&d3, None),
            Err(ResolutionError::InvalidDate)
        ));
    }

    #[test]
    fn test_metadata_extraction() {
        let notes = vec![
            ":Financial:Tax:".to_string(),
            "  Invoice: 1234  ".to_string(),
            "Random comment".to_string(),
        ];
        let (tags, meta) = HIR::resolve_metadata(notes);

        assert_eq!(tags, vec!["Financial", "Tax"]);
        assert_eq!(meta.get("Invoice").unwrap(), "1234");
        // Ensure "Random comment" is discarded (it's neither a tag nor metadata)
        assert_eq!(meta.len(), 1);
    }

    #[test]
    fn test_context_versioning() {
        let mut journal = ast::Journal { entries: vec![] };

        // Setup: Transaction -> Alias Directive -> Transaction
        // We want to ensure Tx1 uses Context 0 and Tx2 uses Context 1.

        let tx_ast = ast::Transaction {
            date: ast::Date {
                year: Some(2024),
                month: 1,
                date: 1,
            },
            description: "Tx".into(),
            ..Default::default()
        };

        journal
            .entries
            .push(ast::Entry::Transaction(tx_ast.clone()));
        journal
            .entries
            .push(ast::Entry::Directive(ast::Directive::Commodity {
                name: "BTC".into(),
                notes: vec![],
                items: vec![ast::CommodityItem::Alias("Bitcoin".into())],
            }));
        journal.entries.push(ast::Entry::Transaction(tx_ast));

        let hir = HIR::try_from(journal).unwrap();

        assert_eq!(hir.contexts.len(), 2);
        assert_eq!(hir.entries[0].context_id, 0);
        assert_eq!(hir.entries[1].context_id, 1);

        // Verify context 1 has the alias
        assert_eq!(
            hir.contexts[1].commodity_aliases.get("Bitcoin").unwrap(),
            "BTC"
        );
        // Verify context 0 does not
        assert!(hir.contexts[0].commodity_aliases.is_empty());
    }
}
