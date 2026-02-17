pub mod ast;
pub mod parser;
pub mod resolution;
pub mod elaboration;

pub use elaboration::Journal;

pub fn compile(mut input: &str) -> Result<elaboration::Journal, Box<dyn std::error::Error>> {
    let output = parser::parse_ledger(&mut input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}
