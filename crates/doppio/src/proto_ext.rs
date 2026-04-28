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
}
