pub mod ast;
pub mod elaboration;
pub mod parser;
pub mod resolution;

pub use elaboration::Journal;

pub fn compile(input: &str) -> Result<elaboration::Journal, Box<dyn std::error::Error>> {
    let output = parser::parse_ledger(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}
