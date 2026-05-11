//! Helpers for navigating colon-separated account-path strings.
//!
//! Account names in doppio (and ledger-cli) use `:` as a hierarchy separator,
//! e.g. `Assets:Bank:Checking`. This module centralises all parsing of that
//! structure so the rest of the CLI does not need to inline ad-hoc colon
//! counting or `rsplit_once` calls.

/// Truncate an account name to at most `depth` colon-separated components.
///
/// Returns a subslice of `account` ending just before the `depth`-th colon,
/// or the full string if it has fewer than `depth` components.
///
/// - `truncate("Expenses:Food:Restaurants", 2)` → `"Expenses:Food"`
/// - `truncate("Assets:Checking", 1)` → `"Assets"`
/// - `truncate("Assets", 1)` → `"Assets"`
pub fn truncate(account: &str, depth: usize) -> &str {
    let mut count = 0;
    for (i, c) in account.char_indices() {
        if c == ':' {
            count += 1;
            if count == depth {
                return &account[..i];
            }
        }
    }
    account
}

/// Return the number of colon-separated segments in the account name.
///
/// Equals the nesting depth in the account tree: a top-level account like
/// `Assets` returns `1`; `Assets:Bank:Checking` returns `3`.
pub fn segment_count(account: &str) -> usize {
    account.chars().filter(|&c| c == ':').count() + 1
}

/// Return the last colon-separated segment of an account name.
///
/// For a top-level account with no colon this is the full string.
///
/// - `last_segment("Assets:Bank:Checking")` → `"Checking"`
/// - `last_segment("Assets")` → `"Assets"`
pub fn last_segment(account: &str) -> &str {
    account.rsplit_once(':').map(|(_, s)| s).unwrap_or(account)
}

/// Return `true` if `maybe_child` is `account` itself or a direct or indirect
/// descendant (i.e. `maybe_child` starts with `account` followed by `:`).
///
/// Prevents false positives such as `"Assets:BankExtra"` matching under
/// `"Assets:Bank"`.
pub fn is_subtree(account: &str, maybe_child: &str) -> bool {
    maybe_child == account
        || maybe_child
            .strip_prefix(account)
            .map(|rest| rest.starts_with(':'))
            .unwrap_or(false)
}

/// Compute the rolled-up per-commodity balance for `account` and all its
/// descendants from the flat `balances` map produced by the balance command.
///
/// Keys in `balances` that equal `account` or start with `account:` are
/// summed. Zero-balance commodities are retained in the output.
pub fn subtree_balance<'a>(
    balances: &'a std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, rust_decimal::Decimal>,
    >,
    account: &str,
) -> std::collections::BTreeMap<&'a str, rust_decimal::Decimal> {
    let mut out: std::collections::BTreeMap<&str, rust_decimal::Decimal> =
        std::collections::BTreeMap::new();
    for (acct, commodities) in balances {
        if is_subtree(account, acct) {
            for (commodity, amount) in commodities {
                *out.entry(commodity.as_str()).or_default() += amount;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn truncate_basic() {
        assert_eq!(truncate("Expenses:Food:Restaurants", 2), "Expenses:Food");
        assert_eq!(truncate("Assets:Checking", 1), "Assets");
        assert_eq!(truncate("Assets", 1), "Assets");
    }

    #[test]
    fn truncate_exact_depth() {
        assert_eq!(truncate("Assets:Bank:Checking", 3), "Assets:Bank:Checking");
    }

    #[test]
    fn segment_count_counts_segments() {
        assert_eq!(segment_count("Assets"), 1);
        assert_eq!(segment_count("Assets:Bank"), 2);
        assert_eq!(segment_count("Assets:Bank:Checking"), 3);
    }

    #[test]
    fn last_segment_extracts_tail() {
        assert_eq!(last_segment("Assets:Bank:Checking"), "Checking");
        assert_eq!(last_segment("Assets:Bank"), "Bank");
        assert_eq!(last_segment("Assets"), "Assets");
    }

    #[test]
    fn is_subtree_recognises_descendants() {
        assert!(is_subtree("Assets:Bank", "Assets:Bank"));
        assert!(is_subtree("Assets:Bank", "Assets:Bank:Checking"));
        assert!(is_subtree("Assets:Bank", "Assets:Bank:Savings"));
        assert!(!is_subtree("Assets:Bank", "Assets:BankExtra"));
        assert!(!is_subtree("Assets:Bank", "Liabilities"));
        assert!(!is_subtree("Assets", "Assets2"));
    }

    #[test]
    fn subtree_balance_sums_descendants() {
        let mut balances: BTreeMap<String, BTreeMap<String, rust_decimal::Decimal>> =
            BTreeMap::new();
        balances
            .entry("Assets:Bank:Checking".to_owned())
            .or_default()
            .insert("USD".to_owned(), rust_decimal::Decimal::from(100));
        balances
            .entry("Assets:Bank:Savings".to_owned())
            .or_default()
            .insert("USD".to_owned(), rust_decimal::Decimal::from(200));

        let result = subtree_balance(&balances, "Assets:Bank");
        assert_eq!(
            result.get("USD").copied(),
            Some(rust_decimal::Decimal::from(300))
        );
    }

    #[test]
    fn subtree_balance_no_false_prefix_match() {
        let mut balances: BTreeMap<String, BTreeMap<String, rust_decimal::Decimal>> =
            BTreeMap::new();
        balances
            .entry("Assets:BankExtra".to_owned())
            .or_default()
            .insert("USD".to_owned(), rust_decimal::Decimal::from(999));

        let result = subtree_balance(&balances, "Assets:Bank");
        assert!(
            result.is_empty(),
            "should not match Assets:BankExtra under Assets:Bank"
        );
    }
}
