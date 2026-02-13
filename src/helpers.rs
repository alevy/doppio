use winnow::{
    ModalResult, Parser,
    ascii::{newline, space0},
    combinator::{alt, peek, repeat},
    token::{literal, take},
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

type Amount = String;

pub(crate) fn amount(input: &mut &str) -> ModalResult<Amount> {
    let mut amount = String::new();
    loop {
        match peek(alt((
            (hard_stop, crate::comment::Comment::parse).value(()),
            newline.value(()),
        )))
        .parse_next(input)
        {
            Err(winnow::error::ErrMode::Backtrack(_)) => {
                let c = take(1usize).parse_next(input)?;
                amount.push_str(c);
            }
            Err(e) => {
                return Err(e);
            }
            Ok(_) => break,
        };
    }
    Ok(amount)
}
