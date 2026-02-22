pub mod ast;
pub mod elaboration;
pub mod parser;
pub mod resolution;

pub use elaboration::Journal;

pub fn file_openner(pattern: &str) -> String {
    use std::io::Read as _;

    let mut buf = String::new();
    for path in glob::glob(pattern).unwrap() {
        std::fs::File::open(path.unwrap())
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
    }
    buf
}

pub fn compile<F>(input: &String, mut parser: parser::Parser<F>) -> Result<elaboration::Journal, Box<dyn std::error::Error>> where F: Fn(&str) -> String {
    let output = parser.parse(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}
