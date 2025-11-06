use winnow::{
    IResult, Parser,
    bytes::{tag, take},
    character::{newline, space0},
    combinator::{peek, value},
    multi::many1,
};

pub(crate) fn empty_line(input: &str) -> IResult<&str, ()> {
    many1(space0.and(newline)).parse(input)
}

pub(crate) fn hard_stop(input: &str) -> IResult<&str, ()> {
    let (input, _) = winnow::branch::alt((
        // two spaces
        tag("  "),
        // a space and a tab
        tag(" \t"),
        // one tab
        tag("\t"),
    ))
    .parse(input)?;
    let (input, _) = space0(input)?;
    Ok((input, ()))
}

type Amount = String;

pub(crate) fn amount(mut input: &str) -> IResult<&str, Amount> {
    let mut amount = String::new();
    input = loop {
        input = match peek(
            value((), hard_stop.and(crate::comment::Comment::parse)).or(value((), newline)),
        )
        .parse(input)
        {
            Err(winnow::error::ErrMode::Backtrack(input)) => {
                let (input, c) = take(1usize).parse(input.input)?;
                amount.push_str(c);
                input
            }
            Err(e) => return Err(e),
            Ok((input, _)) => break input,
        };
    };
    Ok((input, amount))
}
