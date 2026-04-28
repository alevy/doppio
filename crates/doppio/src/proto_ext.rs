//! Inherent-method helpers and trait impls on prost-generated proto types.
//! Lives in a sibling module so prost regeneration (which writes into
//! `OUT_DIR` via the `build.rs` `include!`) doesn't clobber these impls.

use crate::proto;

impl proto::Decimal {
    /// Reconstruct a [`rust_decimal::Decimal`] from this proto-encoded value.
    ///
    /// Equivalent to the free function [`crate::decimal_from_proto`]; available
    /// as an inherent method for ergonomic call sites (`d.to_decimal()` instead
    /// of `decimal_from_proto(&d)`).
    pub fn to_decimal(&self) -> rust_decimal::Decimal {
        crate::decimal_from_proto(self)
    }
}

impl std::fmt::Display for proto::Decimal {
    /// Formats the decimal using the same output as [`rust_decimal::Decimal`]'s
    /// `Display` impl, so precision and sign are consistent with the
    /// elaboration-side type.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.to_decimal(), f)
    }
}

impl proto::Amount {
    /// Iterate `(commodity, decimal)` pairs in this amount.
    ///
    /// Order is unspecified — the underlying `by_commodity` map is a
    /// `HashMap`, and the protobuf spec does not guarantee map field ordering.
    pub fn iter(&self) -> impl Iterator<Item = (&str, rust_decimal::Decimal)> + '_ {
        self.by_commodity
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_decimal()))
    }

    /// Return the decimal value for `commodity`, or `None` if absent.
    pub fn get(&self, commodity: &str) -> Option<rust_decimal::Decimal> {
        self.by_commodity
            .get(commodity)
            .map(proto::Decimal::to_decimal)
    }
}

impl proto::Posting {
    /// Iterate `(commodity, decimal)` pairs across this posting's amount.
    ///
    /// Yields nothing if `self.amount` is `None` or the amount's
    /// `by_commodity` map is empty.
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

#[cfg(test)]
mod tests {
    use crate::{decimal_from_proto, proto};
    use rust_decimal::Decimal;

    fn make_decimal(mantissa_high: i64, mantissa_low: u64, scale: u32) -> proto::Decimal {
        proto::Decimal {
            mantissa_high,
            mantissa_low,
            scale,
        }
    }

    /// Build a `proto::Decimal` from a `rust_decimal::Decimal` so we can
    /// round-trip arbitrary values without the full journal encoding path.
    fn proto_from_decimal(d: Decimal) -> proto::Decimal {
        let mantissa = d.mantissa();
        proto::Decimal {
            mantissa_high: (mantissa >> 64) as i64,
            mantissa_low: mantissa as u64,
            scale: d.scale(),
        }
    }

    /// `proto::Decimal` for 7.58 — mantissa 758, scale 2.
    fn dec_7_58() -> proto::Decimal {
        make_decimal(0, 758, 2)
    }

    /// `proto::Decimal` for 1.23 — mantissa 123, scale 2.
    fn dec_1_23() -> proto::Decimal {
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
        let amount = proto::Amount {
            by_commodity: Default::default(),
        };
        assert_eq!(amount.iter().count(), 0);
    }

    #[test]
    fn amount_iter_single_commodity() {
        let amount = proto::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        let pairs: Vec<_> = amount.iter().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "USD");
        assert_eq!(pairs[0].1, Decimal::new(758, 2));
    }

    #[test]
    fn amount_iter_multi_commodity_all_present() {
        let amount = proto::Amount {
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
        let amount = proto::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        assert_eq!(amount.get("USD"), Some(Decimal::new(758, 2)));
    }

    #[test]
    fn amount_get_miss_returns_none() {
        let amount = proto::Amount {
            by_commodity: [("USD".to_string(), dec_7_58())].into(),
        };
        assert_eq!(amount.get("EUR"), None);
    }

    // ──────────────────────────────────────────────────────────────────────
    // Posting::amounts
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn posting_amounts_none_amount_yields_nothing() {
        let posting = proto::Posting {
            account: "Assets:Cash".to_string(),
            amount: None,
            ..Default::default()
        };
        assert_eq!(posting.amounts().count(), 0);
    }

    #[test]
    fn posting_amounts_some_empty_amount_yields_nothing() {
        let posting = proto::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(proto::Amount {
                by_commodity: Default::default(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amounts().count(), 0);
    }

    #[test]
    fn posting_amounts_single_commodity() {
        let posting = proto::Posting {
            account: "Expenses:Food".to_string(),
            amount: Some(proto::Amount {
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
        let posting = proto::Posting {
            account: "Assets:Brokerage".to_string(),
            amount: Some(proto::Amount {
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
        let posting = proto::Posting {
            account: "Assets:Cash".to_string(),
            amount: None,
            ..Default::default()
        };
        assert_eq!(posting.amount_in("USD"), None);
    }

    #[test]
    fn posting_amount_in_commodity_absent_returns_none() {
        let posting = proto::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(proto::Amount {
                by_commodity: [("USD".to_string(), dec_7_58())].into(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amount_in("EUR"), None);
    }

    #[test]
    fn posting_amount_in_commodity_present_returns_decimal() {
        let posting = proto::Posting {
            account: "Assets:Cash".to_string(),
            amount: Some(proto::Amount {
                by_commodity: [("USD".to_string(), dec_7_58())].into(),
            }),
            ..Default::default()
        };
        assert_eq!(posting.amount_in("USD"), Some(Decimal::new(758, 2)));
    }
}
