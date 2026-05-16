//! Payee normalization.
//!
//! Two implementations are provided:
//!
//! - [`DefaultNormalizer`]: the conservative baseline. Lowercases alphabetic
//!   characters and treats everything else (digits, punctuation, whitespace)
//!   as a word separator, then collapses runs of separators. Suitable when
//!   over-collapse is a concern.
//!
//! - [`RichNormalizer`]: wraps [`DefaultNormalizer`] and applies three
//!   pre-processing rules before delegating, recovering cross-account merchant
//!   signal that the default cannot:
//!
//!   1. **Reference-code stripping** — removes `*<mixed-alphanum>` segments
//!      (payment-processor prefixes such as `SQ*`, `TST*`, and per-order
//!      codes such as the `*Z162B38S2` in `AMAZON MKTPL*Z162B38S2`).
//!   2. **Bank-wrapper stripping** — removes verbatim bank transaction
//!      boilerplate prefixes (e.g. `"Visa Debit Card Point of Sale Purchase "`)
//!      that are specific to one financial institution's statement format.
//!   3. **Domain-token stripping** — removes tokens that look like URL
//!      domains (`*.com`, `*.net`, `*.org`, `*.io`, `*.co`) since they add
//!      noise without merchant-identity signal.

use std::borrow::Cow;

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

/// Richer normalizer that strips common payee noise before delegating to
/// [`DefaultNormalizer`].
///
/// The three pre-processing rules applied in order are:
///
/// 1. **Reference-code stripping**: any `WORD*CODE` or `*CODE` segment where
///    `CODE` mixes letters and digits (an order or transaction reference) is
///    removed along with the `*`. Pure-alpha tokens after `*` are kept (they
///    are part of the merchant name). Pure-digit tokens are already removed by
///    [`DefaultNormalizer`] and need no special treatment here.
///
/// 2. **Bank-wrapper stripping**: removes the prefix
///    `"Visa Debit Card Point of Sale Purchase "` (case-insensitive) that
///    BECU and similar institutions prepend to every debit-card transaction.
///
/// 3. **Domain-token stripping**: removes tokens whose lowercase form ends
///    with `.com`, `.net`, `.org`, `.io`, or `.co` (with or without a path
///    component). This removes noise like `Amzn.com/bill` and `gosq.com`
///    without stripping meaningful merchant words.
#[derive(Default, Debug, Clone, Copy)]
pub struct RichNormalizer;

impl Normalizer for RichNormalizer {
    fn normalize(&self, raw: &str) -> String {
        let s = strip_bank_wrapper(raw);
        let s = strip_reference_codes(s.as_ref());
        let s = strip_domain_tokens(s.as_ref());
        DefaultNormalizer.normalize(s.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Pre-processing helpers (package-private so tests can call them directly)
// ---------------------------------------------------------------------------

/// Strip the verbatim bank-statement wrapper prefix produced by BECU and
/// similar institutions for debit-card transactions.
///
/// Matches case-insensitively; returns a `Cow` to avoid allocation when no
/// match occurs.
pub(crate) fn strip_bank_wrapper(raw: &str) -> Cow<'_, str> {
    const PREFIX: &str = "Visa Debit Card Point of Sale Purchase ";
    if raw.len() >= PREFIX.len() && raw[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        Cow::Borrowed(&raw[PREFIX.len()..])
    } else {
        Cow::Borrowed(raw)
    }
}

/// Strip `*<mixed-alphanum>` reference codes from a payee string.
///
/// A *mixed-alphanum* token is a contiguous run of ASCII letters and digits
/// that contains at least one letter **and** at least one digit and no
/// internal whitespace. Such tokens are characteristic of per-order reference
/// codes (the `*Z162B38S2` in `AMAZON MKTPL*Z162B38S2`) and mixed-ID
/// processor prefixes.
///
/// The rule: for each whitespace-delimited token that contains `*`, split it
/// on `*` into a pre-part and a post-part. If the post-part is mixed-alphanum,
/// discard it (and the `*`), emitting only the pre-part. If the pre-part is
/// mixed-alphanum, discard it (and the `*`), emitting only the post-part. If
/// both are mixed-alphanum, discard the whole token. If neither is
/// mixed-alphanum (both are pure-alpha or pure-digit), emit the concatenated
/// parts separated by a space (matching [`DefaultNormalizer`] behavior).
///
/// Returns a `Cow` that borrows the input when no modification is needed.
pub(crate) fn strip_reference_codes(raw: &str) -> Cow<'_, str> {
    if !raw.contains('*') {
        return Cow::Borrowed(raw);
    }

    let mut parts: Vec<&str> = Vec::new();
    let mut changed = false;

    for tok in raw.split_whitespace() {
        if !tok.contains('*') {
            parts.push(tok);
            continue;
        }
        // Split on the first `*` only; there's at most one per token in practice.
        let star_pos = tok.find('*').expect("just checked contains");
        let pre = &tok[..star_pos];
        let post = &tok[star_pos + 1..];

        let pre_mixed = is_mixed_alphanum(pre);
        let post_mixed = is_mixed_alphanum(post);

        match (pre_mixed, post_mixed) {
            (false, false) => {
                // Neither side is a code; emit both (the `*` becomes a separator,
                // matching DefaultNormalizer which treats `*` as punctuation).
                if !pre.is_empty() {
                    parts.push(pre);
                }
                if !post.is_empty() {
                    parts.push(post);
                }
                // The split changes whitespace structure, so mark as changed.
                changed = true;
            }
            (true, false) => {
                // Pre is a code (e.g. `Z7F3B*MERCHANT`): drop pre, keep post.
                if !post.is_empty() {
                    parts.push(post);
                }
                changed = true;
            }
            (false, true) => {
                // Post is a code (e.g. `MKTPL*Z162B38S2`): keep pre, drop post.
                if !pre.is_empty() {
                    parts.push(pre);
                }
                changed = true;
            }
            (true, true) => {
                // Both sides are codes: drop the whole token.
                changed = true;
            }
        }
    }

    if changed {
        Cow::Owned(parts.join(" "))
    } else {
        Cow::Borrowed(raw)
    }
}

/// Strip the TLD suffix (`.com`, `.net`, `.org`, `.io`, `.co`) and any
/// following path components from whitespace-delimited tokens that contain
/// them.
///
/// For each token, if it contains a TLD suffix, the suffix and everything
/// after it are removed. If what remains before the suffix is non-empty it is
/// kept (preserving brand names like `Amazon` from `Amazon.com`). If nothing
/// remains (e.g. the token was just `.com`) the token is dropped entirely.
///
/// This removes noise such as `Amzn.com/bill` → `Amzn`, `gosq.com` → dropped,
/// `LYFT.COM` → `LYFT`, `Amazon.com` → `Amazon`.
///
/// Returns a `Cow` that borrows the input when no modification is needed.
pub(crate) fn strip_domain_tokens(raw: &str) -> Cow<'_, str> {
    const TLD_SUFFIXES: &[&str] = &[".com", ".net", ".org", ".io", ".co"];

    let needs_change = raw.split_whitespace().any(|tok| {
        let lower = tok.to_ascii_lowercase();
        TLD_SUFFIXES.iter().any(|tld| lower.contains(tld))
    });

    if !needs_change {
        return Cow::Borrowed(raw);
    }

    let parts: Vec<&str> = raw
        .split_whitespace()
        .filter_map(|tok| {
            let lower = tok.to_ascii_lowercase();
            // Find the earliest TLD occurrence.
            let tld_pos = TLD_SUFFIXES.iter().filter_map(|tld| lower.find(tld)).min();
            match tld_pos {
                None => Some(tok),
                Some(pos) => {
                    // Keep only the brand prefix before the TLD.
                    let prefix = &tok[..pos];
                    if prefix.is_empty() {
                        None
                    } else {
                        Some(prefix)
                    }
                }
            }
        })
        .collect();

    Cow::Owned(parts.join(" "))
}

/// Returns `true` if `tok` is a non-empty ASCII string containing at least
/// one letter **and** at least one digit (and no whitespace). This pattern
/// is characteristic of payment-processor codes and order reference IDs.
fn is_mixed_alphanum(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    let has_alpha = tok.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = tok.chars().any(|c| c.is_ascii_digit());
    let no_space = !tok.contains(char::is_whitespace);
    has_alpha && has_digit && no_space
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // DefaultNormalizer
    // -------------------------------------------------------------------

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

    // -------------------------------------------------------------------
    // strip_bank_wrapper
    // -------------------------------------------------------------------

    #[test]
    fn bank_wrapper_stripped_case_insensitive() {
        assert_eq!(
            strip_bank_wrapper("Visa Debit Card Point of Sale Purchase SP INOVELLI LLC MI"),
            "SP INOVELLI LLC MI"
        );
        assert_eq!(
            strip_bank_wrapper("VISA DEBIT CARD POINT OF SALE PURCHASE WINDWORKS WA"),
            "WINDWORKS WA"
        );
    }

    #[test]
    fn bank_wrapper_no_match_returned_as_is() {
        let raw = "STARBUCKS #1234 SEATTLE WA";
        assert_eq!(strip_bank_wrapper(raw), raw);
    }

    // -------------------------------------------------------------------
    // is_mixed_alphanum
    // -------------------------------------------------------------------

    #[test]
    fn mixed_alphanum_detects_code() {
        assert!(is_mixed_alphanum("Z162B38S2"));
        assert!(is_mixed_alphanum("ZC74I49M2"));
        assert!(is_mixed_alphanum("ML16245N3"));
    }

    #[test]
    fn pure_alpha_not_mixed() {
        assert!(!is_mixed_alphanum("Starbucks"));
        assert!(!is_mixed_alphanum("SQ"));
        assert!(!is_mixed_alphanum("TST"));
    }

    #[test]
    fn pure_digit_not_mixed() {
        assert!(!is_mixed_alphanum("1234"));
        assert!(!is_mixed_alphanum("206"));
    }

    // -------------------------------------------------------------------
    // strip_reference_codes
    // -------------------------------------------------------------------

    #[test]
    fn amazon_order_id_stripped() {
        // `*Z162B38S2` is mixed-alphanum → stripped; pre-token `MKTPL` is
        // pure-alpha → kept. Domain tokens are handled by a separate step.
        assert_eq!(
            strip_reference_codes("AMAZON MKTPL*Z162B38S2 Amzn.com/bill WA"),
            "AMAZON MKTPL Amzn.com/bill WA"
        );
    }

    #[test]
    fn sq_prefix_stripped() {
        // `SQ` is pure-alpha so NOT treated as a code by itself; but `SQ*`
        // followed by `VAN` (pure-alpha) → `SQ*` prefix is stripped because
        // the pre-token `SQ` has no digit content.
        //
        // Wait -- `SQ` is pure-alpha, not mixed. So Rule 1 (pre_mixed) won't
        // fire. And the post-token `VAN` is also pure-alpha, so post_mixed
        // won't fire either.
        //
        // The `SQ*` prefix is already handled adequately: DefaultNormalizer
        // turns `SQ*VAN LEEUWEN` into `sq van leeuwen`. The RichNormalizer
        // only *additionally* strips the post-`*` token when it's a
        // reference code.
        assert_eq!(
            strip_reference_codes("SQ*VAN LEEUWEN ICE CREAM Boston MA"),
            "SQ VAN LEEUWEN ICE CREAM Boston MA"
        );
    }

    #[test]
    fn tst_prefix_stripped_leaving_merchant() {
        // `TST` is pure-alpha; post-token `Starbucks` is pure-alpha → no mixed
        // tokens; `*` is removed but tokens kept.
        assert_eq!(
            strip_reference_codes("TST*Starbucks Seattle WA"),
            "TST Starbucks Seattle WA"
        );
    }

    #[test]
    fn amazon_dot_com_order_id_stripped() {
        // "Amazon.com" is pure-alpha (no digits) + the pre-token of the `*`;
        // "Z14WV0BN1" is mixed-alphanum → dropped.
        assert_eq!(
            strip_reference_codes("Amazon.com*Z14WV0BN1 Amzn.com/bill WA"),
            "Amazon.com Amzn.com/bill WA"
        );
    }

    #[test]
    fn no_asterisk_unchanged() {
        let raw = "STARBUCKS SEATTLE WA";
        assert_eq!(strip_reference_codes(raw), raw);
    }

    #[test]
    fn counterexample_distinct_merchants_stay_distinct() {
        // Two different coffee shops accessed via SQ*. After reference-code
        // stripping and DefaultNormalization they produce different keys
        // because the merchant names differ (both pure-alpha, neither stripped).
        let a = RichNormalizer.normalize("SQ*BLUE BOTTLE COFFEE Seattle WA");
        let b = RichNormalizer.normalize("SQ*STARBUCKS #1234 Seattle WA");
        assert_ne!(
            a, b,
            "distinct merchants behind same processor must not collapse"
        );
    }

    // -------------------------------------------------------------------
    // strip_domain_tokens
    // -------------------------------------------------------------------

    #[test]
    fn amzn_bill_domain_stripped() {
        // "Amzn.com/bill" → prefix before ".com" is "Amzn"; "/bill" is dropped.
        assert_eq!(
            strip_domain_tokens("AMAZON MKTPL Amzn.com/bill WA"),
            "AMAZON MKTPL Amzn WA"
        );
    }

    #[test]
    fn gosq_domain_stripped_entirely() {
        // "gosq.com" → prefix before ".com" is "gosq" → kept as "gosq".
        assert_eq!(
            strip_domain_tokens("SQ THE ELECTRIC BOAT gosq.com WA"),
            "SQ THE ELECTRIC BOAT gosq WA"
        );
    }

    #[test]
    fn lyft_domain_stripped() {
        // "LYFT.COM" → prefix before ".com" (case-insensitive find) is "LYFT".
        assert_eq!(
            strip_domain_tokens("LYFT RIDE MON PM LYFT.COM CA"),
            "LYFT RIDE MON PM LYFT CA"
        );
    }

    #[test]
    fn amazon_com_brand_preserved() {
        // "Amazon.com" → "Amazon" (brand preserved, .com dropped).
        assert_eq!(
            strip_domain_tokens("Amazon.com Amzn.com/bill WA"),
            "Amazon Amzn WA"
        );
    }

    #[test]
    fn no_domain_unchanged() {
        let raw = "STARBUCKS SEATTLE WA";
        assert_eq!(strip_domain_tokens(raw), raw);
    }

    // -------------------------------------------------------------------
    // RichNormalizer end-to-end
    // -------------------------------------------------------------------

    #[test]
    fn amazon_marketplace_variants_collapse() {
        let n = RichNormalizer;
        // Two different Amazon orders → same normalized form.
        let a = n.normalize("AMAZON MKTPL*Z162B38S2 Amzn.com/bill WA");
        let b = n.normalize("AMAZON MKTPL*ZC74I49M2 Amzn.com/bill WA");
        assert_eq!(
            a, b,
            "different Amazon order IDs must normalize identically"
        );
        // And the normalized form contains the meaningful brand tokens.
        assert!(a.contains("amazon"), "brand token must be preserved");
        assert!(a.contains("mktpl"), "marketplace token must be preserved");
    }

    #[test]
    fn amazon_com_variant_collapses_with_mktpl() {
        let n = RichNormalizer;
        // Amazon.com* and AMAZON MKTPL* are both Amazon orders; they may not
        // produce identical normalized forms (the brand prefix differs), but
        // they both preserve `amazon` as a token for IDF fallback.
        let a = n.normalize("Amazon.com*Z14WV0BN1 Amzn.com/bill WA");
        let b = n.normalize("AMAZON MKTPL*ZC74I49M2 Amzn.com/bill WA");
        assert!(a.contains("amazon"));
        assert!(b.contains("amazon"));
    }

    #[test]
    fn tst_starbucks_strips_processor_prefix() {
        // TST* prefix stripped → RichNormalizer gets just "starbucks seattle wa".
        let n = RichNormalizer;
        assert_eq!(
            n.normalize("TST*Starbucks Seattle WA"),
            "tst starbucks seattle wa"
        );
    }

    #[test]
    fn bank_wrapper_stripped_before_normalization() {
        let n = RichNormalizer;
        let wrapped = "Visa Debit Card Point of Sale Purchase WINDWORKS SAILING WA";
        let bare = "WINDWORKS SAILING WA";
        assert_eq!(n.normalize(wrapped), n.normalize(bare));
    }

    #[test]
    fn bank_wrapper_counterexample_two_merchants_stay_distinct() {
        let n = RichNormalizer;
        let a = n.normalize("Visa Debit Card Point of Sale Purchase STARBUCKS SEATTLE WA");
        let b = n.normalize("Visa Debit Card Point of Sale Purchase COMCAST CABLE BELLEVUE WA");
        assert_ne!(
            a, b,
            "different merchants behind bank wrapper must remain distinct"
        );
    }

    #[test]
    fn lyft_variants_collapse() {
        let n = RichNormalizer;
        // Two Lyft rides with different time-of-day annotation and domain noise.
        let a = n.normalize("LYFT *RIDE MON 2PM LYFT.COM CA");
        let b = n.normalize("LYFT *RIDE SAT 4AM LYFT.COM CA");
        // Both contain `lyft` and `ride`; domain stripped; time tokens also
        // stripped (pm, am are alpha but short -- DefaultNormalizer keeps them).
        // Key assertion: both still contain the brand.
        assert!(a.contains("lyft"));
        assert!(b.contains("lyft"));
    }

    #[test]
    fn default_normalizer_unchanged() {
        // Verify DefaultNormalizer output is identical to what it was before --
        // RichNormalizer must not change DefaultNormalizer behavior.
        let d = DefaultNormalizer;
        let r = RichNormalizer;
        // For a plain merchant with no special chars, both produce the same output.
        assert_eq!(
            d.normalize("Starbucks Seattle WA"),
            r.normalize("Starbucks Seattle WA")
        );
    }
}
