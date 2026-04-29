//! Payee normalization.
//!
//! v0.1 ships a single implementation, [`DefaultNormalizer`], that lowercases
//! alphabetic characters and treats every other character (digits,
//! punctuation, whitespace) as a word separator, then collapses runs of
//! separators into a single space.
//!
//! v0.2 will introduce token-IDF normalization for cases the default
//! normalizer cannot handle: "starbucks seattle wa" / "starbucks portland or"
//! (same vendor, different locations) and "gusto payroll" / "gusto taxes"
//! (same prefix, different services).

/// Strategy for normalizing raw payee strings into a comparison key.
pub trait Normalizer: Send + Sync {
    /// Normalize a raw payee string.
    fn normalize(&self, raw: &str) -> String;
}

/// Default normalizer: lowercase alphabetic characters; treat everything
/// else as a word separator; collapse runs of separators.
#[derive(Default, Debug, Clone, Copy)]
pub struct DefaultNormalizer;

impl Normalizer for DefaultNormalizer {
    fn normalize(&self, raw: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        let mut prev_space = true;
        for ch in raw.chars() {
            if ch.is_alphabetic() {
                for lower in ch.to_lowercase() {
                    out.push(lower);
                }
                prev_space = false;
            } else if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        }
        if out.ends_with(' ') {
            out.pop();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases() {
        assert_eq!(DefaultNormalizer.normalize("STARBUCKS"), "starbucks");
    }

    #[test]
    fn strips_digits_with_word_boundary() {
        assert_eq!(
            DefaultNormalizer.normalize("STARBUCKS #1234 SEATTLE WA"),
            "starbucks seattle wa"
        );
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(
            DefaultNormalizer.normalize("  Multi   Spaces  "),
            "multi spaces"
        );
    }

    #[test]
    fn punctuation_is_separator() {
        assert_eq!(
            DefaultNormalizer.normalize("AMZN MKTP US*ABC123"),
            "amzn mktp us abc"
        );
    }

    #[test]
    fn tst_prefix_separates() {
        assert_eq!(
            DefaultNormalizer.normalize("TST*Starbucks 4567"),
            "tst starbucks"
        );
    }

    #[test]
    fn unicode_letters_preserved() {
        assert_eq!(DefaultNormalizer.normalize("Café 123"), "café");
    }

    #[test]
    fn starbucks_variants_collide_when_location_matches() {
        let n = DefaultNormalizer;
        assert_eq!(
            n.normalize("STARBUCKS #1234 SEATTLE WA"),
            "starbucks seattle wa"
        );
        assert_eq!(n.normalize("Starbucks Seattle, WA"), "starbucks seattle wa");
    }

    #[test]
    fn locations_differ_v01_limitation() {
        // v0.1 limitation: different locations normalize to different strings.
        // v0.2 token-IDF will collapse these.
        let n = DefaultNormalizer;
        assert_ne!(
            n.normalize("Starbucks Seattle WA"),
            n.normalize("Starbucks Portland OR")
        );
    }

    #[test]
    fn empty_string() {
        assert_eq!(DefaultNormalizer.normalize(""), "");
    }

    #[test]
    fn only_digits() {
        assert_eq!(DefaultNormalizer.normalize("12345"), "");
    }

    #[test]
    fn trailing_separator_trimmed() {
        assert_eq!(DefaultNormalizer.normalize("foo bar "), "foo bar");
        assert_eq!(DefaultNormalizer.normalize("foo bar 123"), "foo bar");
    }
}
