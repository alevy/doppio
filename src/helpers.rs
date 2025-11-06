use winnow::{
    IResult, Parser,
    ascii::{newline, space0},
    combinator::{alt, peek, repeat},
    token::{tag, take},
};

pub(crate) fn empty_line(input: &str) -> IResult<&str, ()> {
    repeat(1.., (space0, newline)).parse_next(input)
}

pub(crate) fn hard_stop(input: &str) -> IResult<&str, ()> {
    let (input, _) = alt((
        // two spaces
        tag("  "),
        // a space and a tab
        tag(" \t"),
        // one tab
        tag("\t"),
    ))
    .parse_next(input)?;
    let (input, _) = space0(input)?;
    Ok((input, ()))
}

type Amount = String;

pub(crate) fn amount(mut input: &str) -> IResult<&str, Amount> {
    let mut amount = String::new();
    input = loop {
        input = match peek(alt((
            (hard_stop, crate::comment::Comment::parse).value(()),
            newline.value(()),
        )))
        .parse_next(input)
        {
            Err(winnow::error::ErrMode::Backtrack(input)) => {
                let (input, c) = take(1usize).parse_next(input.input)?;
                amount.push_str(c);
                input
            }
            Err(e) => return Err(e),
            Ok((input, _)) => break input,
        };
    };
    Ok((input, amount))
}
