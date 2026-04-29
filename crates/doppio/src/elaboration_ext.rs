//! Inherent-method helpers and trait impls on prost-generated proto types.
//! Lives in a sibling module so prost regeneration (which writes into
//! `OUT_DIR` via the `build.rs` `include!`) doesn't clobber these impls.

use crate::elaboration;
use std::collections::{BTreeMap, HashSet, VecDeque};

impl elaboration::Decimal {
    /// Reconstruct a [`rust_decimal::Decimal`] from this proto-encoded value.
    ///
    /// Equivalent to the free function [`crate::decimal_from_proto`]; available
    /// as an inherent method for ergonomic call sites (`d.to_decimal()` instead
    /// of `decimal_from_proto(&d)`).
    pub fn to_decimal(&self) -> rust_decimal::Decimal {
        crate::decimal_from_proto(self)
    }
}

impl std::fmt::Display for elaboration::Decimal {
    /// Formats the decimal using the same output as [`rust_decimal::Decimal`]'s
    /// `Display` impl, so precision and sign are consistent with the
    /// elaboration-side type.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.to_decimal(), f)
    }
}

impl elaboration::Amount {
    /// Iterate `(commodity, decimal)` pairs in this amount.
    ///
    /// Pairs are yielded in commodity-symbol order: the underlying
    /// `by_commodity` field is a `BTreeMap` (configured via prost's
    /// `btree_map` build option in `build.rs`), so iteration is
    /// deterministic and sorted. Note that this guarantee is specific to
    /// doppio's Rust binding — bindings in other languages may iterate
    /// protobuf maps in unspecified order per the protobuf spec.
    pub fn iter(&self) -> impl Iterator<Item = (&str, rust_decimal::Decimal)> + '_ {
        self.by_commodity
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_decimal()))
    }

    /// Return the decimal value for `commodity`, or `None` if absent.
    pub fn get(&self, commodity: &str) -> Option<rust_decimal::Decimal> {
        self.by_commodity
            .get(commodity)
            .map(elaboration::Decimal::to_decimal)
    }
}

impl elaboration::Posting {
    /// Returns the posting kind, mapping the prost-generated `i32` to the
    /// [`elaboration::PostingKind`] enum.
    ///
    /// Unknown wire values (i.e. values that do not map to any `PostingKind`
    /// variant) are mapped to `Real` via the `unwrap_or` fallback, preserving
    /// forward-compatibility. `UNSPECIFIED` (wire value 0, field absent in
    /// older `.dop` files) passes through as `PostingKind::Unspecified` and is
    /// treated as real by [`Self::is_real`] — so pre-#140 archives that don't
    /// carry this field are read as all-real without a format version bump.
    pub fn posting_kind(&self) -> elaboration::PostingKind {
        elaboration::PostingKind::try_from(self.kind).unwrap_or(elaboration::PostingKind::Real)
    }

    /// Returns `true` iff this posting is a "real" (non-virtual) posting.
    ///
    /// Both `POSTING_KIND_UNSPECIFIED` and `POSTING_KIND_REAL` are treated as
    /// real. Older `.dop` files that don't carry the `kind` field (field value
    /// = 0 = UNSPECIFIED) are therefore treated as all-real on read — no format
    /// version bump required.
    pub fn is_real(&self) -> bool {
        matches!(
            self.posting_kind(),
            elaboration::PostingKind::Unspecified | elaboration::PostingKind::Real
        )
    }

    /// Return this posting's amount, treating an absent amount field as an
    /// empty [`elaboration::Amount`].
    ///
    /// `elaboration::Posting.amount` is `Option<Amount>` because proto3 wraps
    /// every nested message in `Option`, but in practice doppio's elaboration
    /// stage always populates `amount: Some(_)` — null postings are filled
    /// in during balancing rather than left as `None`. This accessor papers
    /// over the proto3 quirk so consumers don't need to thread `Option`
    /// through every call site, while still being defensive: a malformed
    /// wire payload with `amount = None` produces an empty `Amount` rather
    /// than a panic.
    pub fn amount(&self) -> &elaboration::Amount {
        static EMPTY_AMOUNT: elaboration::Amount = elaboration::Amount {
            by_commodity: std::collections::BTreeMap::new(),
        };
        match self.amount.as_ref() {
            Some(a) => a,
            None => &EMPTY_AMOUNT,
        }
    }

    /// Iterate `(commodity, decimal)` pairs across this posting's amount.
    ///
    /// Yields nothing if `self.amount` is `None` or the amount's
    /// `by_commodity` map is empty. Pairs are yielded in commodity-symbol
    /// order — see [`elaboration::Amount::iter`] for the iteration-order
    /// guarantee.
    pub fn amounts(&self) -> impl Iterator<Item = (&str, rust_decimal::Decimal)> + '_ {
        self.amount.iter().flat_map(|a| {
            a.by_commodity
                .iter()
                .map(|(k, v)| (k.as_str(), v.to_decimal()))
        })
    }

    /// Return the decimal value for `commodity` in this posting's amount,
    /// or `None` if the amount field is absent or `commodity` is not present.
    pub fn amount_in(&self, commodity: &str) -> Option<rust_decimal::Decimal> {
        self.amount.as_ref()?.get(commodity)
    }
}

impl elaboration::Transaction {
    /// Convert the epoch-days `date` field to a [`chrono::NaiveDate`].
    ///
    /// The wire format stores transaction dates as `i32` epoch days
    /// (1970-01-01 = 0, negative for pre-epoch). This method returns the
    /// corresponding `NaiveDate`.
    pub fn date_naive(&self) -> chrono::NaiveDate {
        epoch_days_to_naive_date(self.date)
    }

    /// Convert the optional `secondary_date` field to a [`chrono::NaiveDate`].
    ///
    /// Returns `None` if the secondary date was not set.
    pub fn secondary_date_naive(&self) -> Option<chrono::NaiveDate> {
        self.secondary_date.map(epoch_days_to_naive_date)
    }
}

impl elaboration::HistoricalPrice {
    /// Convert the epoch-days `date` field to a [`chrono::NaiveDate`].
    ///
    /// The wire format stores historical-price dates as `i32` epoch days
    /// (1970-01-01 = 0, negative for pre-epoch).
    pub fn date_naive(&self) -> chrono::NaiveDate {
        epoch_days_to_naive_date(self.date)
    }
}

impl elaboration::Journal {
    /// Return the conversion rate from `from_commodity` to `to_commodity` as
    /// of `as_of` (or the latest available if `as_of` is `None`).
    ///
    /// Searches `self.prices` for `P` entries and performs a BFS through the
    /// commodity price graph to find a conversion path. For each pair of
    /// commodities the most recent quote whose date is `<= as_of` (or the
    /// most recent overall when `as_of` is `None`) is used.
    ///
    /// Multi-hop conversion is supported: if there is no direct EUR→USD quote
    /// but there are EUR→GBP and GBP→USD quotes, the combined rate is returned.
    /// Rates are multiplied along the path (BFS finds shortest hops first).
    ///
    /// Inverse quotes are also traversed: if only USD→EUR is declared, then
    /// EUR→USD is available as `1 / (USD→EUR rate)`. Explicit and derived
    /// (inverse) quotes are merged by the most-recent-date rule.
    ///
    /// When multiple shortest paths exist, the one whose intermediate
    /// commodities sort first alphabetically is used (BFS expansion follows
    /// the BTreeMap-keyed adjacency map's deterministic key order).
    ///
    /// Conversion uses the report's `--end` date as the as-of cutoff, or the
    /// latest available quote if `--end` is not specified. ledger-cli converts
    /// per-posting using the transaction's own date by default; this
    /// implementation uses a single uniform as-of for all postings, which is
    /// simpler but means historical reports without `--end` will use
    /// anachronistically recent rates.
    ///
    /// Returns `None` if no conversion path exists.
    pub fn exchange_rate_at(
        &self,
        from_commodity: &str,
        to_commodity: &str,
        as_of: Option<chrono::NaiveDate>,
    ) -> Option<rust_decimal::Decimal> {
        use rust_decimal::Decimal;

        if from_commodity == to_commodity {
            return Some(Decimal::ONE);
        }

        // Eligible quotes: each `HistoricalPrice` with a non-zero price and a
        // date at or before `as_of` (or any date if `as_of` is `None`),
        // flattened to `(from, to, rate, date)` tuples.
        let eligible_quotes = self.prices.iter().filter_map(|hp| {
            let price_val = hp.price.as_ref()?.to_decimal();
            if price_val == Decimal::ZERO {
                return None;
            }
            let date = hp.date_naive();
            if as_of.is_some_and(|cutoff| date > cutoff) {
                return None;
            }
            Some((
                hp.commodity.as_str(),
                hp.price_commodity.as_str(),
                price_val,
                date,
            ))
        });

        // Build adjacency map: commodity → { neighbour → (date, rate) }.
        // For each eligible quote, insert both a forward edge `from → to` at
        // `rate` and an inverse edge `to → from` at `1/rate`. When the same
        // (from, to) pair appears more than once, keep the most recent quote
        // by date.
        let mut adj: BTreeMap<&str, BTreeMap<&str, (chrono::NaiveDate, Decimal)>> = BTreeMap::new();
        let mut upsert = |from, to, date: chrono::NaiveDate, rate: Decimal| {
            let entry = adj
                .entry(from)
                .or_default()
                .entry(to)
                .or_insert((date, rate));
            if date > entry.0 {
                *entry = (date, rate);
            }
        };
        for (from, to, rate, date) in eligible_quotes {
            upsert(from, to, date, rate);
            upsert(to, from, date, Decimal::ONE / rate);
        }

        // BFS from `from_commodity` to `to_commodity`.
        // State: (current_commodity, accumulated_rate).
        let mut queue: VecDeque<(&str, Decimal)> = VecDeque::new();
        let mut visited: HashSet<&str> = HashSet::new();

        queue.push_back((from_commodity, Decimal::ONE));
        visited.insert(from_commodity);

        while let Some((current, rate)) = queue.pop_front() {
            if let Some(neighbours) = adj.get(current) {
                for (neighbour, (_date, edge_rate)) in neighbours {
                    if visited.contains(neighbour) {
                        continue;
                    }
                    let combined = rate * edge_rate;
                    if *neighbour == to_commodity {
                        return Some(combined);
                    }
                    visited.insert(neighbour);
                    queue.push_back((neighbour, combined));
                }
            }
        }

        None
    }
}

/// Convert epoch days (1970-01-01 = 0, negative for pre-epoch) to a
/// `chrono::NaiveDate`. Internal helper for the `date_naive()` accessors.
fn epoch_days_to_naive_date(days: i32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date")
        + chrono::TimeDelta::days(days as i64)
}

#[cfg(test)]
mod tests {
    use crate::{decimal_from_proto, elaboration};
    use rust_decimal::Decimal;

    fn make_decimal(mantissa_high: i64, mantissa_low: u64, scale: u32) -> elaboration::Decimal {
        elaboration::Decimal {
            mantissa_high,
            mantissa_low,
            scale,
        }
    }

    /// Build a `elaboration::Decimal` from a `rust_decimal::Decimal` so we can
    /// round-trip arbitrary values without the full journal encoding path.
    fn proto_from_decimal(d: Decimal) -> elaboration::Decimal {
        let mantissa = d.mantissa();
        elaboration::Decimal {
            mantissa_high: (mantissa >> 64) as i64,
            mantissa_low: mantissa as u64,
            scale: d.scale(),
        }
    }

    /// `elaboration::Decimal` for 7.58 — mantissa 758, scale 2.
    fn dec_7_58() -> elaboration::Decimal {
        make_decimal(0, 758, 2)
    }

    /// `elaboration::Decimal` for 1.23 — mantissa 123, scale 2.
    fn dec_1_23() -> elaboration::Decimal {
        make_decimal(0, 123, 2)
    }

    // ──────────────────────────────────────────────────────────────────────
    // Decimal::to_decimal
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn to_decimal_positive() {
        // $7.58 → mantissa 758, scale 2
        let d = make_decimal(0, 758, 2);
        let expected = Decimal::new(758, 2);
        assert_eq!(d.to_decimal(), expected);
    }

    #[test]
    fn to_decimal_parity_with_free_function() {
        // Verify the method and the free function always agree.
        let d = make_decimal(0, 758, 2);
        assert_eq!(d.to_decimal(), decimal_from_proto(&d));
    }

    #[test]
    fn to_decimal_negative() {
        // -1.23: mantissa = -123 as i128, scale = 2.
        let neg: i128 = -123;
        let mantissa_high = (neg >> 64) as i64;
        let mantissa_low = neg as u64;
        let d = make_decimal(mantissa_high, mantissa_low, 2);
        let expected = Decimal::new(-123, 2);
        assert_eq!(d.to_decimal(), expected);
        assert_eq!(d.to_decimal(), decimal_from_proto(&d));
    }

    #[test]
    fn to_decimal_zero() {
        let d = make_decimal(0, 0, 0);
        assert_eq!(d.to_decimal(), Decimal::ZERO);
        assert_eq!(d.to_decimal(), decimal_from_proto(&d));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Decimal: Display
    // ──────────────────────────────────────────────────────────────────────

    /// The Display contract: `format!("{}", proto_decimal)` must produce
    /// exactly the same string as `format!("{}", rust_decimal::Decimal)` for
    /// the same numeric value.
    #[test]
    fn display_matches_rust_decimal() {
        let cases: &[(&str, Decimal)] = &[
            ("positive whole", Decimal::from(100u32)),
            ("positive fractional", "7.58".parse().unwrap()),
            ("negative fractional", "-7.58".parse().unwrap()),
            ("zero", Decimal::ZERO),
        ];

        for (label, expected) in cases {
            let pd = proto_from_decimal(*expected);
            assert_eq!(
                format!("{pd}"),
                format!("{expected}"),
                "Display mismatch for {label}"
            );
        }
    }

    /// Pin the exact strings produced so that any future change to the
    /// output format surfaces as a test failure.
    #[test]
    fn display_exact_strings() {
        let cases: &[(&str, &str)] = &[
            ("7.58", "7.58"),
            ("-7.58", "-7.58"),
            ("0", "0"),
            ("100", "100"),
        ];

        for (input, want) in cases {
            let d: Decimal = input.parse().unwrap();
            let pd = proto_from_decimal(d);
            assert_eq!(format!("{pd}"), *want, "unexpected Display for {input}");
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // Amount::iter
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn amount_iter_empty_map_yields_nothing() {
        let amount = elaboration::Amount {
            by_commodity: Default::default(),
        };
        assert_eq!(amount.iter().count(), 0);
    }

    #[test]
    fn amount_iter_single_commodity() {
        let amount = elaboration::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        let pairs: Vec<_> = amount.iter().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "USD");
        assert_eq!(pairs[0].1, Decimal::new(758, 2));
    }

    #[test]
    fn amount_iter_multi_commodity_all_present() {
        let amount = elaboration::Amount {
            by_commodity: [
                ("USD".to_string(), dec_7_58()),
                ("EUR".to_string(), dec_1_23()),
            ]
            .into(),
        };
        let mut pairs: Vec<_> = amount.iter().collect();
        // Sort so the assertion is order-independent.
        pairs.sort_by_key(|(c, _)| *c);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("EUR", Decimal::new(123, 2)));
        assert_eq!(pairs[1], ("USD", Decimal::new(758, 2)));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Amount::get
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn amount_get_hit_returns_decimal() {
        let amount = elaboration::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        assert_eq!(amount.get("USD"), Some(Decimal::new(758, 2)));
    }

    #[test]
    fn amount_get_miss_returns_none() {
        let amount = elaboration::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        assert_eq!(amount.get("EUR"), None);
    }

    // ──────────────────────────────────────────────────────────────────────
    // Posting::amounts
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn posting_amounts_none_amount_yields_nothing() {
        let posting = elaboration::Posting {
            account: "Assets:Cash".to_string(),
            amount: None,
            ..Default::default()
        };
        assert_eq!(posting.amounts().count(), 0);
    }

    #[test]
    fn posting_amounts_some_empty_amount_yields_nothing() {
        let posting = elaboration::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(elaboration::Amount {
                by_commodity: Default::default(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amounts().count(), 0);
    }

    #[test]
    fn posting_amounts_single_commodity() {
        let posting = elaboration::Posting {
            account: "Expenses:Food".to_string(),
            amount: Some(elaboration::Amount {
                by_commodity: [("USD".to_string(), dec_7_58())].into(),
            }),
            ..Default::default()
        };
        let pairs: Vec<_> = posting.amounts().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "USD");
        assert_eq!(pairs[0].1, Decimal::new(758, 2));
    }

    #[test]
    fn posting_amounts_multi_commodity() {
        let posting = elaboration::Posting {
            account: "Assets:Brokerage".to_string(),
            amount: Some(elaboration::Amount {
                by_commodity: [
                    ("USD".to_string(), dec_7_58()),
                    ("EUR".to_string(), dec_1_23()),
                ]
                .into(),
            }),
            ..Default::default()
        };
        let mut pairs: Vec<_> = posting.amounts().collect();
        pairs.sort_by_key(|(c, _)| *c);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("EUR", Decimal::new(123, 2)));
        assert_eq!(pairs[1], ("USD", Decimal::new(758, 2)));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Posting::amount_in
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn posting_amount_in_none_amount_returns_none() {
        let posting = elaboration::Posting {
            account: "Assets:Cash".to_string(),
            amount: None,
            ..Default::default()
        };
        assert_eq!(posting.amount_in("USD"), None);
    }

    #[test]
    fn posting_amount_in_commodity_absent_returns_none() {
        let posting = elaboration::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(elaboration::Amount {
                by_commodity: [("USD".to_string(), dec_7_58())].into(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amount_in("EUR"), None);
    }

    #[test]
    fn posting_amount_in_commodity_present_returns_decimal() {
        let posting = elaboration::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(elaboration::Amount {
                by_commodity: [("USD".to_string(), dec_7_58())].into(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amount_in("USD"), Some(Decimal::new(758, 2)));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Transaction::date_naive / secondary_date_naive
    // HistoricalPrice::date_naive
    // ──────────────────────────────────────────────────────────────────────

    use chrono::NaiveDate;

    fn epoch_days(year: i32, month: u32, day: u32) -> i32 {
        let date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        (date - epoch).num_days() as i32
    }

    #[test]
    fn transaction_date_naive_epoch() {
        let txn = elaboration::Transaction {
            date: 0,
            ..Default::default()
        };
        assert_eq!(
            txn.date_naive(),
            NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
        );
    }

    #[test]
    fn transaction_date_naive_positive() {
        let txn = elaboration::Transaction {
            date: epoch_days(2024, 1, 15),
            ..Default::default()
        };
        assert_eq!(
            txn.date_naive(),
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
        );
    }

    #[test]
    fn transaction_date_naive_pre_epoch() {
        let txn = elaboration::Transaction {
            date: epoch_days(1969, 6, 30),
            ..Default::default()
        };
        assert_eq!(
            txn.date_naive(),
            NaiveDate::from_ymd_opt(1969, 6, 30).unwrap()
        );
    }

    #[test]
    fn transaction_secondary_date_naive_some() {
        let txn = elaboration::Transaction {
            date: epoch_days(2024, 1, 15),
            secondary_date: Some(epoch_days(2024, 2, 1)),
            ..Default::default()
        };
        assert_eq!(
            txn.secondary_date_naive(),
            Some(NaiveDate::from_ymd_opt(2024, 2, 1).unwrap())
        );
    }

    #[test]
    fn transaction_secondary_date_naive_none() {
        let txn = elaboration::Transaction {
            date: epoch_days(2024, 1, 15),
            secondary_date: None,
            ..Default::default()
        };
        assert_eq!(txn.secondary_date_naive(), None);
    }

    #[test]
    fn historical_price_date_naive() {
        let hp = elaboration::HistoricalPrice {
            date: epoch_days(2024, 3, 4),
            ..Default::default()
        };
        assert_eq!(
            hp.date_naive(),
            NaiveDate::from_ymd_opt(2024, 3, 4).unwrap()
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // Posting::is_real / Posting::posting_kind
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn is_real_kind_zero_unspecified_treated_as_real() {
        let p = elaboration::Posting {
            kind: 0,
            ..Default::default()
        };
        assert!(p.is_real(), "UNSPECIFIED (0) must be treated as real");
    }

    #[test]
    fn is_real_kind_one_real_is_real() {
        let p = elaboration::Posting {
            kind: 1,
            ..Default::default()
        };
        assert!(p.is_real());
    }

    #[test]
    fn is_real_kind_two_virtual_unbalanced_is_not_real() {
        let p = elaboration::Posting {
            kind: 2,
            ..Default::default()
        };
        assert!(!p.is_real());
    }

    #[test]
    fn is_real_kind_three_virtual_balanced_is_not_real() {
        let p = elaboration::Posting {
            kind: 3,
            ..Default::default()
        };
        assert!(!p.is_real());
    }

    #[test]
    fn posting_kind_zero_returns_unspecified() {
        let p = elaboration::Posting {
            kind: 0,
            ..Default::default()
        };
        assert_eq!(p.posting_kind(), elaboration::PostingKind::Unspecified);
    }

    #[test]
    fn posting_kind_one_returns_real() {
        let p = elaboration::Posting {
            kind: 1,
            ..Default::default()
        };
        assert_eq!(p.posting_kind(), elaboration::PostingKind::Real);
    }

    #[test]
    fn posting_kind_two_returns_virtual_unbalanced() {
        let p = elaboration::Posting {
            kind: 2,
            ..Default::default()
        };
        assert_eq!(
            p.posting_kind(),
            elaboration::PostingKind::VirtualUnbalanced
        );
    }

    // Journal::exchange_rate_at
    // ──────────────────────────────────────────────────────────────────────

    /// Build a minimal `HistoricalPrice` proto entry.
    fn make_price(
        year: i32,
        month: u32,
        day: u32,
        commodity: &str,
        price_commodity: &str,
        amount: Decimal,
    ) -> elaboration::HistoricalPrice {
        let mantissa = amount.mantissa();
        elaboration::HistoricalPrice {
            date: epoch_days(year, month, day),
            commodity: commodity.to_string(),
            price_commodity: price_commodity.to_string(),
            price: Some(elaboration::Decimal {
                mantissa_high: (mantissa >> 64) as i64,
                mantissa_low: mantissa as u64,
                scale: amount.scale(),
            }),
            ..Default::default()
        }
    }

    /// Construct a `Journal` with only the given price entries.
    fn journal_with_prices(prices: Vec<elaboration::HistoricalPrice>) -> elaboration::Journal {
        elaboration::Journal {
            prices,
            ..Default::default()
        }
    }

    #[test]
    fn exchange_rate_at_same_commodity_returns_one() {
        let j = journal_with_prices(vec![]);
        assert_eq!(
            j.exchange_rate_at("USD", "USD", None),
            Some(Decimal::ONE),
            "same commodity → rate 1"
        );
    }

    #[test]
    fn exchange_rate_at_direct_quote() {
        let j = journal_with_prices(vec![make_price(
            2024,
            1,
            1,
            "EUR",
            "$",
            "1.10".parse().unwrap(),
        )]);
        let rate = j
            .exchange_rate_at("EUR", "$", None)
            .expect("direct quote present");
        assert_eq!(rate, "1.10".parse::<Decimal>().unwrap());
    }

    #[test]
    fn exchange_rate_at_inverse_quote() {
        // Only USD→EUR declared; querying EUR→USD should give 1/rate.
        let j = journal_with_prices(vec![make_price(
            2024,
            1,
            1,
            "USD",
            "EUR",
            "0.9".parse().unwrap(),
        )]);
        let rate = j
            .exchange_rate_at("EUR", "USD", None)
            .expect("inverse path exists");
        // 1 / 0.9 ≈ 1.111...
        let expected = Decimal::ONE / "0.9".parse::<Decimal>().unwrap();
        assert_eq!(rate, expected);
    }

    #[test]
    fn exchange_rate_at_two_hop_chain() {
        // EUR→GBP and GBP→USD; no direct EUR→USD.
        let j = journal_with_prices(vec![
            make_price(2024, 1, 1, "EUR", "GBP", "0.85".parse().unwrap()),
            make_price(2024, 1, 1, "GBP", "USD", "1.27".parse().unwrap()),
        ]);
        let rate = j
            .exchange_rate_at("EUR", "USD", None)
            .expect("two-hop path exists");
        let expected = "0.85".parse::<Decimal>().unwrap() * "1.27".parse::<Decimal>().unwrap();
        assert_eq!(rate, expected);
    }

    #[test]
    fn exchange_rate_at_no_path_returns_none() {
        // EUR→GBP exists, but there is no path to USD.
        let j = journal_with_prices(vec![make_price(
            2024,
            1,
            1,
            "EUR",
            "GBP",
            "0.85".parse().unwrap(),
        )]);
        assert_eq!(j.exchange_rate_at("EUR", "USD", None), None);
    }

    #[test]
    fn exchange_rate_at_as_of_filtering_excludes_future_quote() {
        // Quote dated 2024-01-01; as_of is 2023-12-31 — should not be visible.
        let j = journal_with_prices(vec![make_price(
            2024,
            1,
            1,
            "EUR",
            "$",
            "1.10".parse().unwrap(),
        )]);
        let cutoff = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        assert_eq!(
            j.exchange_rate_at("EUR", "$", Some(cutoff)),
            None,
            "future quote must be invisible to past as_of"
        );
    }

    #[test]
    fn posting_kind_three_returns_virtual_balanced() {
        let p = elaboration::Posting {
            kind: 3,
            ..Default::default()
        };
        assert_eq!(p.posting_kind(), elaboration::PostingKind::VirtualBalanced);
    }

    #[test]
    fn posting_kind_unknown_wire_value_falls_back_to_real() {
        // An unknown wire value (e.g. from a future format version) should fall
        // back to Real rather than panicking.
        let p = elaboration::Posting {
            kind: 99,
            ..Default::default()
        };
        assert_eq!(p.posting_kind(), elaboration::PostingKind::Real);
        assert!(p.is_real(), "unknown wire values must be treated as real");
    }

    #[test]
    fn exchange_rate_at_as_of_uses_most_recent_eligible_quote() {
        // Three quotes: 1.05 on 2024-01-01, 1.10 on 2024-03-01, and 1.20 on
        // 2024-12-01 (after the cutoff).  as_of=2024-06-01 must pick the
        // 2024-03-01 quote (1.10) over the earlier 2024-01-01 quote (1.05),
        // exercising the most-recent-wins update branch, and must not see the
        // future 2024-12-01 quote.
        let j = journal_with_prices(vec![
            make_price(2024, 1, 1, "EUR", "$", "1.05".parse().unwrap()),
            make_price(2024, 3, 1, "EUR", "$", "1.10".parse().unwrap()),
            make_price(2024, 12, 1, "EUR", "$", "1.20".parse().unwrap()),
        ]);
        let cutoff = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let rate = j
            .exchange_rate_at("EUR", "$", Some(cutoff))
            .expect("eligible quote exists");
        assert_eq!(rate, "1.10".parse::<Decimal>().unwrap());
    }

    #[test]
    fn exchange_rate_at_no_prices_returns_none() {
        let j = journal_with_prices(vec![]);
        assert_eq!(j.exchange_rate_at("EUR", "USD", None), None);
    }

    #[test]
    fn exchange_rate_at_cross_directional_quote_collision_most_recent_wins() {
        // P 2024-01-01 A B 1.0   → explicit A→B at 1.0, derived B→A at 1.0
        // P 2024-06-01 B A 0.8   → explicit B→A at 0.8 (newer), derived A→B at 1/0.8 = 1.25
        //
        // The more recent B→A quote (0.8) beats the older explicit A→B quote
        // (1.0) because both the explicit and derived (inverse) edges compete
        // on the same (A→B / B→A) adjacency slots and the most-recent-date
        // rule applies uniformly.  exchange_rate_at("A","B",None) should therefore
        // return 1/0.8 = 1.25, not 1.0.
        let j = journal_with_prices(vec![
            make_price(2024, 1, 1, "A", "B", "1.0".parse().unwrap()),
            make_price(2024, 6, 1, "B", "A", "0.8".parse().unwrap()),
        ]);
        let expected = Decimal::ONE / "0.8".parse::<Decimal>().unwrap();
        let rate = j
            .exchange_rate_at("A", "B", None)
            .expect("path exists via inverse of B→A");
        assert_eq!(rate, expected);
    }
}
