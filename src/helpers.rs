use winnow::{
    ascii::{newline, space0}, combinator::{alt, peek, repeat}, token::{tag, take}, PResult, Parser
};

pub(crate) fn empty_line(input: &mut &str) -> PResult<()> {
    repeat(1.., (space0, newline)).parse_next(input)
}

pub(crate) fn hard_stop(input: &mut &str) -> PResult<()> {
    alt((
        // two spaces
        tag("  "),
        // a space and a tab
        tag(" \t"),
        // one tab
        tag("\t"),
    ))
    .parse_next(input)?;
    space0(input)?;
    Ok(())
}

type Amount = String;

pub(crate) fn amount(input: &mut &str) -> PResult<Amount> {
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
            Err(e) => return Err(e),
            Ok(_) => break,
        };
    };
    Ok(amount)
}
