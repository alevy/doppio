//! Beancount frontend (experimental): parse `.beancount` source text.
//!
//! This module currently ships only the pest-derived parser. The
//! `Frontend` trait impl and the AST adapter that lowers a parsed
//! Beancount file into [`crate::ast::Journal`] are tracked in #146.
//! The `pad`-directive evaluator is tracked in #147.
//!
//! Marked **experimental** in line with the Beancount milestone (M#9):
//! the directive set, lot syntax, and string-escape handling will
//! evolve as real Beancount inputs surface gaps.

use pest_derive::Parser;

/// The raw pest parser generated from `beancount.pest` via `pest_derive`.
///
/// Internal-only: the public Beancount surface lands with the AST
/// adapter in #146 (a `BeancountFrontend` implementing
/// [`crate::frontend::Frontend`]). Until then, this struct is only
/// referenced from `#[cfg(test)]` grammar-smoke-test code, so the
/// non-test build flags it as dead -- legitimately so. The allow
/// is removed when #146 wires the parser into a Frontend impl.
#[allow(dead_code)]
#[derive(Parser)]
#[grammar = "grammars/beancount/beancount.pest"]
pub(crate) struct BeancountParser;

#[cfg(test)]
mod tests {
    use super::*;
    use pest::Parser as _;

    /// The fixture covers every directive type the grammar supports;
    /// it is the contract for #145.
    const SAMPLE: &str = include_str!("../../../tests/fixtures/sample.beancount");

    #[test]
    fn sample_fixture_parses() {
        BeancountParser::parse(Rule::journal, SAMPLE).unwrap_or_else(|e| {
            panic!("sample.beancount failed to parse:\n{e}");
        });
    }

    fn parse_one(rule: Rule, input: &str) {
        BeancountParser::parse(rule, input)
            .unwrap_or_else(|e| panic!("expected `{input}` to match {rule:?}, got: {e}"));
    }

    #[test]
    fn date_iso_only() {
        parse_one(Rule::date, "2024-01-15");
    }

    #[test]
    fn date_rejects_slash_separator() {
        // Beancount is strict about ISO format. hledger's `2024/01/15`
        // is not accepted.
        assert!(BeancountParser::parse(Rule::date, "2024/01/15").is_err());
    }

    #[test]
    fn currency_uppercase_identifier() {
        parse_one(Rule::commodity, "USD");
        parse_one(Rule::commodity, "EUR");
        parse_one(Rule::commodity, "AAPL");
        parse_one(Rule::commodity, "BTC");
        parse_one(Rule::commodity, "VHT_2024");
    }

    #[test]
    fn currency_must_start_uppercase() {
        // Beancount currencies are uppercase-led. Lowercase 'usd' is
        // not a currency token; it would parse as something else.
        assert!(BeancountParser::parse(Rule::commodity, "usd").is_err());
    }

    #[test]
    fn account_colon_segments() {
        parse_one(Rule::account, "Assets:Bank:Checking");
        parse_one(Rule::account, "Equity:Opening-Balances");
        parse_one(Rule::account, "Income:US:Acme:Salary");
    }

    #[test]
    fn account_requires_at_least_two_segments() {
        // Beancount accounts always have at least two segments
        // (Type:Name minimum). A bare "Assets" is not a valid
        // account token.
        assert!(BeancountParser::parse(Rule::account, "Assets").is_err());
    }

    #[test]
    fn string_double_quoted() {
        parse_one(Rule::string, "\"hello world\"");
        parse_one(Rule::string, "\"\"");
    }

    #[test]
    fn flag_recognises_star_bang_txn() {
        parse_one(Rule::flag, "*");
        parse_one(Rule::flag, "!");
        parse_one(Rule::flag, "txn");
    }

    #[test]
    fn tag_and_link_chars() {
        parse_one(Rule::tag, "#vacation");
        parse_one(Rule::tag, "#trip-2024");
        parse_one(Rule::link, "^statement-2024-01");
    }

    #[test]
    fn plugin_directive_with_and_without_arg() {
        // Covered here rather than in sample.beancount because real
        // Beancount plugins are signature-versioned and bean-check
        // refuses the fixture if a referenced plugin's API has shifted.
        parse_one(Rule::plugin_directive, "plugin \"beancount.plugins.foo\"\n");
        parse_one(
            Rule::plugin_directive,
            "plugin \"beancount.plugins.foo\" \"argument-string\"\n",
        );
    }
}
