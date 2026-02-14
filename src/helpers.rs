use winnow::{
    ModalResult, Parser,
    ascii::{newline, space0},
    combinator::{alt, repeat},
    token::literal,
};

pub(crate) fn empty_line(input: &mut &str) -> ModalResult<()> {
    repeat(1.., (space0, newline)).parse_next(input)
}

pub(crate) fn hard_stop(input: &mut &str) -> ModalResult<()> {
    alt((
        // two spaces
        literal("  "),
        // a space and a tab
        literal(" \t"),
        // one tab
        literal("\t"),
    ))
    .parse_next(input)?;
    space0(input)?;
    Ok(())
}
