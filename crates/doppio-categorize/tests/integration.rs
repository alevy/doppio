//! Integration tests that build small `doppio::elaboration::Journal` fixtures
//! by hand (using the prost-generated proto shape) and exercise the full
//! `Index::build` / `Index::suggest` path.

use chrono::NaiveDate;
use doppio::elaboration::{
    Amount, Decimal as ProtoDecimal, Journal, Posting, Transaction, TransactionState,
};
use doppio_categorize::{Config, DefaultNormalizer, Index, Query, ScoringStrategy};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;

fn epoch_days(year: i32, month: u32, day: u32) -> i32 {
    let date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    (date - epoch).num_days() as i32
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn proto_decimal(d: Decimal) -> ProtoDecimal {
    let mantissa = d.mantissa();
    ProtoDecimal {
        mantissa_low: mantissa as u64,
        mantissa_high: (mantissa >> 64) as i64,
        scale: d.scale(),
    }
}

fn amount(d: Decimal, commodity: &str) -> Amount {
    let mut by_commodity = BTreeMap::new();
    by_commodity.insert(commodity.to_string(), proto_decimal(d));
    Amount { by_commodity }
}

fn posting(account: &str, payee: &str, amt: Decimal) -> Posting {
    Posting {
        account: account.to_string(),
        payee: payee.to_string(),
        amount: Some(amount(amt, "$")),
        state: TransactionState::Uncleared as i32,
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        ..Default::default()
    }
}

fn txn(date_ymd: (i32, u32, u32), description: &str, postings: Vec<Posting>) -> Transaction {
    Transaction {
        date: epoch_days(date_ymd.0, date_ymd.1, date_ymd.2),
        secondary_date: None,
        state: TransactionState::Uncleared as i32,
        code: None,
        description: description.to_string(),
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        postings,
    }
}

fn empty_journal() -> Journal {
    Journal::default()
}

#[test]
fn empty_journal_returns_no_suggestions() {
    let j = empty_journal();
    let idx = Index::build(&j, DefaultNormalizer);
    let q = Query {
        date: date(2024, 1, 15),
        payee: "Starbucks".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:Visa".into(),
    };
    assert!(idx.suggest(&q, &Config::default()).is_empty());
}

#[test]
fn single_payee_single_account_confidence_one() {
    let mut j = empty_journal();
    for day in 1..=10 {
        j.transactions.push(txn(
            (2024, 1, day),
            "Starbucks",
            vec![
                posting("Liabilities:Visa", "Starbucks", dec!(-7.58)),
                posting("Expenses:Coffee", "Starbucks", dec!(7.58)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:Visa".into(),
    };
    let s = idx.suggest(&q, &Config::default());
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].account, "Expenses:Coffee");
    assert!((s[0].confidence - 1.0).abs() < 1e-9);
    assert_eq!(s[0].sample_count, 10);
    assert_eq!(s[0].last_seen, date(2024, 1, 10));
}

#[test]
fn two_payee_history_concentrates_on_dominant_counter() {
    // 8 Starbucks -> Coffee, 2 Local Coffee -> Coffee. All on Visa.
    // Querying with payee "Starbucks" should only see Starbucks samples;
    // sample_count=8, confidence=1.0 (single counter_account).
    let mut j = empty_journal();
    for day in 1..=8 {
        j.transactions.push(txn(
            (2024, 1, day),
            "Starbucks",
            vec![
                posting("Liabilities:Visa", "Starbucks", dec!(-7.58)),
                posting("Expenses:Coffee", "Starbucks", dec!(7.58)),
            ],
        ));
    }
    for day in 9..=10 {
        j.transactions.push(txn(
            (2024, 1, day),
            "Local Coffee",
            vec![
                posting("Liabilities:Visa", "Local Coffee", dec!(-5.00)),
                posting("Expenses:Coffee", "Local Coffee", dec!(5.00)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:Visa".into(),
    };
    let s = idx.suggest(&q, &Config::default());
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].account, "Expenses:Coffee");
    assert_eq!(s[0].sample_count, 8);
}

#[test]
fn pcc_amount_split_picks_correct_cluster() {
    // Half the history: PCC $14 -> Expenses:PreparedFoods (lunch from prepared bar)
    // Half the history: PCC $200 -> Expenses:Groceries
    let mut j = empty_journal();
    for day in 1..=8 {
        j.transactions.push(txn(
            (2024, 1, day),
            "PCC",
            vec![
                posting("Liabilities:Visa", "PCC", dec!(-14.00)),
                posting("Expenses:PreparedFoods", "PCC", dec!(14.00)),
            ],
        ));
    }
    for day in 9..=16 {
        j.transactions.push(txn(
            (2024, 1, day),
            "PCC",
            vec![
                posting("Liabilities:Visa", "PCC", dec!(-200.00)),
                posting("Expenses:Groceries", "PCC", dec!(200.00)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);

    // Query at $14 -> PreparedFoods rank 1
    let q14 = Query {
        date: date(2024, 2, 1),
        payee: "PCC".into(),
        amount: dec!(-14.00),
        known_account: "Liabilities:Visa".into(),
    };
    let s14 = idx.suggest(&q14, &Config::default());
    assert_eq!(s14.len(), 2, "both clusters should contribute, but ranked");
    assert_eq!(s14[0].account, "Expenses:PreparedFoods");
    assert!(
        s14[0].confidence > s14[1].confidence,
        "PreparedFoods should outrank Groceries at $14: {:?}",
        s14
    );

    // Query at $200 -> Groceries rank 1
    let q200 = Query {
        date: date(2024, 2, 1),
        payee: "PCC".into(),
        amount: dec!(-200.00),
        known_account: "Liabilities:Visa".into(),
    };
    let s200 = idx.suggest(&q200, &Config::default());
    assert_eq!(s200.len(), 2);
    assert_eq!(s200[0].account, "Expenses:Groceries");
    assert!(
        s200[0].confidence > s200[1].confidence,
        "Groceries should outrank PreparedFoods at $200: {:?}",
        s200
    );
}

#[test]
fn sign_filter_separates_charges_and_refunds() {
    // 5 charges: Visa -7.58 -> Expenses:Coffee
    // 1 refund: Visa +7.58 -> Income:Refunds
    let mut j = empty_journal();
    for day in 1..=5 {
        j.transactions.push(txn(
            (2024, 1, day),
            "Starbucks",
            vec![
                posting("Liabilities:Visa", "Starbucks", dec!(-7.58)),
                posting("Expenses:Coffee", "Starbucks", dec!(7.58)),
            ],
        ));
    }
    j.transactions.push(txn(
        (2024, 1, 10),
        "Starbucks",
        vec![
            posting("Liabilities:Visa", "Starbucks", dec!(7.58)),
            posting("Income:Refunds", "Starbucks", dec!(-7.58)),
        ],
    ));
    let idx = Index::build(&j, DefaultNormalizer);

    // Refund query (positive amount on Visa side)
    let q_refund = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks".into(),
        amount: dec!(7.58),
        known_account: "Liabilities:Visa".into(),
    };
    let s_refund = idx.suggest(&q_refund, &Config::default());
    assert_eq!(s_refund.len(), 1, "only refund samples should survive");
    assert_eq!(s_refund[0].account, "Income:Refunds");

    // Charge query (negative amount on Visa side)
    let q_charge = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:Visa".into(),
    };
    let s_charge = idx.suggest(&q_charge, &Config::default());
    assert_eq!(s_charge.len(), 1, "only charge samples should survive");
    assert_eq!(s_charge[0].account, "Expenses:Coffee");
}

#[test]
fn cold_start_returns_empty() {
    let mut j = empty_journal();
    j.transactions.push(txn(
        (2024, 1, 1),
        "Starbucks",
        vec![
            posting("Liabilities:Visa", "Starbucks", dec!(-7.58)),
            posting("Expenses:Coffee", "Starbucks", dec!(7.58)),
        ],
    ));
    let idx = Index::build(&j, DefaultNormalizer);
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Brand New Vendor".into(),
        amount: dec!(-50.00),
        known_account: "Liabilities:Visa".into(),
    };
    assert!(idx.suggest(&q, &Config::default()).is_empty());
}

#[test]
fn unknown_known_account_returns_empty() {
    let mut j = empty_journal();
    j.transactions.push(txn(
        (2024, 1, 1),
        "Starbucks",
        vec![
            posting("Liabilities:Visa", "Starbucks", dec!(-7.58)),
            posting("Expenses:Coffee", "Starbucks", dec!(7.58)),
        ],
    ));
    let idx = Index::build(&j, DefaultNormalizer);
    // Same payee, different known_account
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:OtherCard".into(),
    };
    assert!(idx.suggest(&q, &Config::default()).is_empty());
}

#[test]
fn payee_normalization_collides_starbucks_variants() {
    let mut j = empty_journal();
    j.transactions.push(txn(
        (2024, 1, 1),
        "STARBUCKS #1234 SEATTLE WA",
        vec![
            posting(
                "Liabilities:Visa",
                "STARBUCKS #1234 SEATTLE WA",
                dec!(-7.58),
            ),
            posting("Expenses:Coffee", "STARBUCKS #1234 SEATTLE WA", dec!(7.58)),
        ],
    ));
    let idx = Index::build(&j, DefaultNormalizer);
    // Different store number, different formatting -- same normalized payee.
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks #5678 Seattle WA".into(),
        amount: dec!(-7.58),
        known_account: "Liabilities:Visa".into(),
    };
    let s = idx.suggest(&q, &Config::default());
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].account, "Expenses:Coffee");
}

#[test]
fn use_amount_weighting_disabled_makes_clusters_equal() {
    // Same as pcc_amount_split, but with use_amount_weighting=false.
    // Without amount weighting, both clusters tie on the $14 query
    // (both contribute weight 1.0 per sample, equal counts).
    let mut j = empty_journal();
    for day in 1..=8 {
        j.transactions.push(txn(
            (2024, 1, day),
            "PCC",
            vec![
                posting("Liabilities:Visa", "PCC", dec!(-14.00)),
                posting("Expenses:PreparedFoods", "PCC", dec!(14.00)),
            ],
        ));
    }
    for day in 9..=16 {
        j.transactions.push(txn(
            (2024, 1, day),
            "PCC",
            vec![
                posting("Liabilities:Visa", "PCC", dec!(-200.00)),
                posting("Expenses:Groceries", "PCC", dec!(200.00)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);

    let q14 = Query {
        date: date(2024, 2, 1),
        payee: "PCC".into(),
        amount: dec!(-14.00),
        known_account: "Liabilities:Visa".into(),
    };
    let cfg = Config {
        use_amount_weighting: false,
        ..Config::default()
    };
    let s = idx.suggest(&q14, &cfg);
    assert_eq!(s.len(), 2);
    // With equal counts and uniform weight, confidences are equal.
    assert!((s[0].confidence - s[1].confidence).abs() < 1e-9);
}

#[test]
fn token_idf_rescues_unseen_payee_variant() {
    // Training: Starbucks Seattle WA, multiple times.
    // Query: Starbucks Portland OR -- no exact normalized match.
    // Default Hybrid strategy should fall back to token-IDF, which sees
    // the shared "starbucks" token (rare-ish, since the rest of the corpus
    // doesn't use it) and recovers the right counter.
    let mut j = empty_journal();
    for day in 1..=10 {
        j.transactions.push(txn(
            (2024, 1, day),
            "STARBUCKS SEATTLE WA",
            vec![
                posting("Liabilities:Visa", "STARBUCKS SEATTLE WA", dec!(-7.58)),
                posting("Expenses:Coffee", "STARBUCKS SEATTLE WA", dec!(7.58)),
            ],
        ));
    }
    // A few unrelated transactions on the same known_account, to test that
    // token-IDF doesn't accidentally match on common tokens.
    for day in 11..=15 {
        j.transactions.push(txn(
            (2024, 1, day),
            "Comcast Cable",
            vec![
                posting("Liabilities:Visa", "Comcast Cable", dec!(-89.95)),
                posting("Expenses:Internet", "Comcast Cable", dec!(89.95)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);

    // Variant query -- "starbucks portland or" doesn't appear in training.
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Starbucks Portland OR".into(),
        amount: dec!(-6.42),
        known_account: "Liabilities:Visa".into(),
    };

    // Under ExactMatch strategy, this is a cold-start: nothing matches.
    let cfg_exact = Config {
        strategy: ScoringStrategy::ExactMatch,
        ..Config::default()
    };
    assert!(
        idx.suggest(&q, &cfg_exact).is_empty(),
        "ExactMatch should not find a Portland variant"
    );

    // Under default Hybrid, token-IDF rescues it via the shared "starbucks" token.
    let s = idx.suggest(&q, &Config::default());
    assert!(!s.is_empty(), "Hybrid should recover via token-IDF");
    assert_eq!(s[0].account, "Expenses:Coffee");
}

#[test]
fn token_idf_filters_high_df_tokens() {
    // Three distinct vendors:
    //   - "MEGA RETAILER SEATTLE WA" (10x) -> Shopping
    //   - "RESTAURANT A SEATTLE WA"   (2x)  -> Dining
    //   - "GUSTO PAYROLL"             (5x)  -> Wages (no Seattle)
    //
    // Distinct normalized payees: 3. The token "seattle" appears in 2 of 3
    // (df=2), as does "wa" (df=2). The non-geographic tokens have df=1.
    let mut j = empty_journal();
    for day in 1..=10 {
        j.transactions.push(txn(
            (2024, 1, day),
            "MEGA RETAILER SEATTLE WA",
            vec![
                posting("Liabilities:Visa", "MEGA RETAILER SEATTLE WA", dec!(-50.00)),
                posting("Expenses:Shopping", "MEGA RETAILER SEATTLE WA", dec!(50.00)),
            ],
        ));
    }
    for day in 11..=12 {
        j.transactions.push(txn(
            (2024, 1, day),
            "RESTAURANT A SEATTLE WA",
            vec![
                posting("Liabilities:Visa", "RESTAURANT A SEATTLE WA", dec!(-30.00)),
                posting("Expenses:Dining", "RESTAURANT A SEATTLE WA", dec!(30.00)),
            ],
        ));
    }
    for day in 13..=17 {
        j.transactions.push(txn(
            (2024, 1, day),
            "GUSTO PAYROLL",
            vec![
                posting("Liabilities:Visa", "GUSTO PAYROLL", dec!(2000.00)),
                posting("Income:Wages", "GUSTO PAYROLL", dec!(-2000.00)),
            ],
        ));
    }
    let idx = Index::build(&j, DefaultNormalizer);

    // Query: a brand-new vendor in Seattle. The only tokens it shares with
    // training are "seattle" and "wa".
    let q = Query {
        date: date(2024, 2, 1),
        payee: "BRAND NEW SHOP SEATTLE WA".into(),
        amount: dec!(-15.00),
        known_account: "Liabilities:Visa".into(),
    };

    // df_threshold=2: filters "seattle" and "wa" (df=2 >= threshold).
    // No other shared tokens -- cold-start.
    let cfg_strict = Config {
        strategy: ScoringStrategy::TokenIdf { df_threshold: 2 },
        ..Config::default()
    };
    assert!(
        idx.suggest(&q, &cfg_strict).is_empty(),
        "geographic-only overlap should be filtered by a tight df threshold"
    );

    // df_threshold=3: includes "seattle" and "wa" (df=2 < 3). They do have
    // non-zero IDF (ln(3/2) ≈ 0.405 each) since gusto-payroll lacks them.
    // Query then matches 12 samples (10 mega + 2 restaurant). Mega's
    // counter (Shopping) wins on volume.
    let cfg_loose = Config {
        strategy: ScoringStrategy::TokenIdf { df_threshold: 3 },
        ..Config::default()
    };
    let s = idx.suggest(&q, &cfg_loose);
    assert!(
        !s.is_empty(),
        "with a permissive df threshold, geographic tokens propagate matches"
    );
}

#[test]
fn posting_with_no_amount_is_skipped() {
    // proto3 makes Posting.amount Option<Amount>. doppio elaboration always
    // populates Some, but a hand-built or wire-decoded fixture could carry a
    // None. Index::build must tolerate that gracefully (skip the posting,
    // don't panic) since `posting.amounts()` yields nothing in that case.
    let mut j = empty_journal();
    j.transactions.push(Transaction {
        date: epoch_days(2024, 1, 1),
        secondary_date: None,
        state: TransactionState::Uncleared as i32,
        code: None,
        description: "Mystery".to_string(),
        tags: Vec::new(),
        metadata: BTreeMap::new(),
        postings: vec![
            Posting {
                account: "Liabilities:Visa".to_string(),
                payee: "Mystery".to_string(),
                amount: None,
                state: TransactionState::Uncleared as i32,
                tags: Vec::new(),
                metadata: BTreeMap::new(),
                ..Default::default()
            },
            posting("Expenses:Unknown", "Mystery", dec!(10.00)),
        ],
    });
    // Add a normal-shaped Mystery transaction so there is something for the
    // index to learn from.
    j.transactions.push(txn(
        (2024, 1, 2),
        "Mystery",
        vec![
            posting("Liabilities:Visa", "Mystery", dec!(-10.00)),
            posting("Expenses:Misc", "Mystery", dec!(10.00)),
        ],
    ));
    let idx = Index::build(&j, DefaultNormalizer);
    let q = Query {
        date: date(2024, 2, 1),
        payee: "Mystery".into(),
        amount: dec!(-10.00),
        known_account: "Liabilities:Visa".into(),
    };
    let s = idx.suggest(&q, &Config::default());
    // Only the second transaction's Visa posting contributes a known sample
    // for "Liabilities:Visa". The first transaction's Visa-side amount was
    // None and must have been skipped.
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].account, "Expenses:Misc");
    assert_eq!(s[0].sample_count, 1);
}
