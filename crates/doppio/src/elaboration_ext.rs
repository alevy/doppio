//! Inherent-method helpers and trait impls on prost-generated proto types.
//! Lives in a sibling module so prost regeneration (which writes into
//! `OUT_DIR` via the `build.rs` `include!`) doesn't clobber these impls.

use crate::elaboration;

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
    /// `POSTING_KIND_UNSPECIFIED` (field absent in older `.dop` files) is
    /// mapped to `POSTING_KIND_REAL`, preserving backward compatibility:
    /// pre-#140 archives that don't carry this field are treated as all-real.
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
}
