/// Workload generators for benchmarks.
///
/// Each function returns a String of ledger-format text that can be fed
/// directly to the parser. Sizes are chosen to keep each benchmark group
/// well under a second so the full suite is fast enough for pre-commit use.

/// A plain two-posting transaction: one expense account, one asset account.
/// Represents the most common real-world pattern.
pub fn simple(n: usize) -> String {
    let accounts = [
        ("Expenses:Food", "Assets:Bank:Checking"),
        ("Expenses:Transport", "Assets:Bank:Checking"),
        ("Expenses:Utilities", "Assets:Bank:Checking"),
        ("Expenses:Shopping", "Liabilities:CreditCard"),
        ("Income:Salary", "Assets:Bank:Checking"),
    ];
    let mut out = String::with_capacity(n * 80);
    for i in 0..n {
        let (debit, credit) = accounts[i % accounts.len()];
        let amount = 10 + (i % 490);
        let day = (i % 28) + 1;
        let month = (i % 12) + 1;
        let year = 2020 + i / 336;
        out.push_str(&format!(
            "{:04}/{:02}/{:02} Payee {}\n    {}  $ {}\n    {}\n\n",
            year, month, day, i % 100, amount, debit, credit,
        ));
    }
    out
}

/// Many distinct accounts — stresses account-name interning and BTreeMap
/// lookups during elaboration.
pub fn wide(n: usize) -> String {
    let mut out = String::with_capacity(n * 90);
    for i in 0..n {
        let acct = format!("Expenses:Category{}", i % 500);
        let amount = 10 + (i % 490);
        let day = (i % 28) + 1;
        let month = (i % 12) + 1;
        let year = 2020 + i / 336;
        out.push_str(&format!(
            "{:04}/{:02}/{:02} Payee {}\n    {}  $ {}\n    Assets:Bank:Checking\n\n",
            year, month, day, i % 100, acct, amount,
        ));
    }
    out
}

/// Five postings per transaction — stresses balance resolution and the
/// implicit-amount inference path.
pub fn deep(n: usize) -> String {
    let accounts = [
        "Expenses:Food",
        "Expenses:Transport",
        "Expenses:Utilities",
        "Expenses:Shopping",
    ];
    let mut out = String::with_capacity(n * 200);
    for i in 0..n {
        let day = (i % 28) + 1;
        let month = (i % 12) + 1;
        let year = 2020 + i / 336;
        out.push_str(&format!(
            "{:04}/{:02}/{:02} Payee {}\n",
            year, month, day, i % 100
        ));
        for (j, acct) in accounts.iter().enumerate() {
            out.push_str(&format!("    {}  $ {}\n", acct, 10 + (i + j) % 90));
        }
        // Final posting has implicit amount (sum of the above, negated)
        out.push_str("    Assets:Bank:Checking\n\n");
    }
    out
}

/// Multiple commodities per transaction — stresses Amount handling and
/// commodity string interning.
pub fn multi_commodity(n: usize) -> String {
    let commodities = ["USD", "EUR", "GBP"];
    let mut out = String::with_capacity(n * 120);
    for i in 0..n {
        let commodity = commodities[i % commodities.len()];
        let amount = 10 + (i % 490);
        let day = (i % 28) + 1;
        let month = (i % 12) + 1;
        let year = 2020 + i / 336;
        out.push_str(&format!(
            "{:04}/{:02}/{:02} Payee {}\n    Expenses:Foreign  {} {}\n    Assets:Bank:Checking  {} -{}\n\n",
            year, month, day, i % 100, commodity, amount, commodity, amount,
        ));
    }
    out
}

/// All workloads paired with a name and the default transaction count.
pub fn workloads() -> Vec<(&'static str, String)> {
    vec![
        ("simple", simple(10_000)),
        ("wide", wide(10_000)),
        ("deep", deep(10_000)),
        ("multi_commodity", multi_commodity(10_000)),
    ]
}
