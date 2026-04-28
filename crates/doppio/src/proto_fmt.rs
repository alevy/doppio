//! Display impls for prost-generated proto types. Lives in a sibling
//! module so prost regeneration (which writes into OUT_DIR via the
//! build.rs include!) doesn't clobber these impls.

use crate::proto;

impl std::fmt::Display for proto::Decimal {
    /// Formats the decimal using the same output as [`rust_decimal::Decimal`]'s
    /// `Display` impl, so precision and sign are consistent with the
    /// elaboration-side type.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegate to rust_decimal::Decimal's existing Display impl so
        // formatting (precision, sign) is consistent with the elaboration-
        // side type.
        std::fmt::Display::fmt(&crate::decimal_from_proto(self), f)
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::proto;

    /// Build a `proto::Decimal` directly from a `rust_decimal::Decimal` so
    /// we can round-trip through Display without going via the full journal
    /// encoding path.
    fn proto_from_decimal(d: Decimal) -> proto::Decimal {
        // Mirror the private `decimal_to_proto` logic in lib.rs.
        let mantissa = d.mantissa();
        proto::Decimal {
            mantissa_high: (mantissa >> 64) as i64,
            mantissa_low: mantissa as u64,
            scale: d.scale(),
        }
    }

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
