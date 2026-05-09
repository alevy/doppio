//! Snapshot regression tests for synthesised postings (#235).
//!
//! doppio's elaborator synthesises two classes of postings that have no
//! canonical-tool analogue and therefore cannot be parity-tested:
//!
//! 1. **Capital-gains synthesis** (hledger frontend): when a sale has both a
//!    lot cost and an `@price`, doppio synthesises a posting on the configured
//!    `gains_account` (default `"Income:Capital Gains"`) so the elaborated
//!    journal is cost-basis-balanced.
//!
//! 2. **Rounding-residual synthesis** (#198): when a transaction's per-commodity
//!    residual is within tolerance, doppio absorbs it into a synthesised posting
//!    on the empty-string account `""`.
//!
//! For each positive-parity fixture we:
//!   - Run `dop register --format=json` on the fixture.
//!   - Filter postings to those on `""` (rounding-residual) or
//!     `"Income:Capital Gains"` (gains).
//!   - Sort by `(date, account, commodity)`.
//!   - Serialise as `YYYY-MM-DD | account | commodity | amount` lines.
//!   - Compare against the checked-in snapshot file under `tests/snapshots/`.
//!
//! ## Regenerating snapshots
//!
//! When elaboration behaviour changes deliberately (e.g. a tolerance tweak
//! or a gains-account rename), regenerate the snapshots with:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test snapshot_synthesised
//! ```
//!
//! This writes the current output as the new snapshot without failing. Then
//! review the diff with `git diff` before committing.

use std::{
    env,
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

/// Path to the workspace root, determined relative to this file's location.
fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is set to the crate directory at compile time.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/doppio-cli -> workspace root is two levels up.
    manifest
        .parent()
        .expect("crate has parent dir")
        .parent()
        .expect("crates/ has parent dir")
        .to_owned()
}

/// Path to the snapshot directory.
fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

/// Path to the `dop` binary built for tests.
fn dop_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dop"))
}

/// Run `dop register --format=json` on `fixture` and return the stdout.
fn run_register_json(fixture: &Path) -> String {
    let out = Command::new(dop_bin())
        .args(["register", "--format=json"])
        .arg(fixture)
        .output()
        .expect("failed to execute dop");
    if !out.status.success() {
        panic!(
            "dop exited with {} for {}\nstderr: {}",
            out.status,
            fixture.display(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    String::from_utf8(out.stdout).expect("non-UTF-8 stdout")
}

/// Extract synthesised postings from JSON output and return them sorted.
///
/// A "synthesised" posting is one whose account is either:
/// - `""` — rounding-residual sentinel (see #198)
/// - `"Income:Capital Gains"` — gains-synthesis sentinel (see #210)
///
/// Returns a sorted `Vec<(date, account, commodity, amount)>`.
fn extract_synthesised(json: &str) -> Vec<(String, String, String, String)> {
    let payload: Vec<serde_json::Value> =
        serde_json::from_str(json).expect("valid JSON from dop register");

    let mut rows: Vec<(String, String, String, String)> = payload
        .into_iter()
        .filter_map(|row| {
            let account = row["account"].as_str()?.to_owned();
            let is_rounding_residual = account.is_empty();
            let is_gains = account == "Income:Capital Gains";
            if !is_rounding_residual && !is_gains {
                return None;
            }
            let date = row["date"].as_str()?.to_owned();
            let commodity = row["commodity"].as_str()?.to_owned();
            let amount = row["amount"].as_str()?.to_owned();
            Some((date, account, commodity, amount))
        })
        .collect();

    // Deterministic order: sort by (date, account, commodity, amount).
    // Multiple transactions on the same date can each produce a residual on
    // the same (account, commodity), so amount is included as the final
    // tiebreaker to keep the snapshot fully deterministic.
    rows.sort_by(|a, b| (&a.0, &a.1, &a.2, &a.3).cmp(&(&b.0, &b.1, &b.2, &b.3)));

    rows
}

/// Serialise synthesised rows into snapshot text.
///
/// Format: one line per posting — `YYYY-MM-DD | account | commodity | amount`.
/// An empty fixture produces an empty string (no trailing newline).
fn serialise(rows: &[(String, String, String, String)]) -> String {
    rows.iter()
        .map(|(date, account, commodity, amount)| {
            format!("{date} | {account} | {commodity} | {amount}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Core assertion: compare synthesised postings against the stored snapshot.
///
/// If `UPDATE_SNAPSHOTS=1` is set in the environment, write the current output
/// to the snapshot file instead of comparing. This never fails; the intent is
/// to let `git diff` surface the change for deliberate review.
fn assert_snapshot(fixture_label: &str, fixture: &Path) {
    let json = run_register_json(fixture);
    let rows = extract_synthesised(&json);
    let actual = serialise(&rows);

    let snap_path = snapshot_dir().join(format!("{fixture_label}.snap"));

    if env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        // Regeneration mode: overwrite the snapshot. Ensure the directory exists.
        std::fs::create_dir_all(snapshot_dir()).expect("create snapshots directory");
        let mut f = std::fs::File::create(&snap_path).unwrap_or_else(|e| {
            panic!(
                "failed to create snapshot file {}: {e}",
                snap_path.display()
            )
        });
        f.write_all(actual.as_bytes()).expect("write snapshot file");
        if !actual.is_empty() {
            // Trailing newline for non-empty snapshots so `git diff` is clean.
            f.write_all(b"\n").expect("write trailing newline");
        }
        println!(
            "updated snapshot: {} ({} rows)",
            snap_path.display(),
            rows.len()
        );
        return;
    }

    // Comparison mode: the snapshot file must already exist.
    let stored = std::fs::read_to_string(&snap_path).unwrap_or_else(|e| {
        panic!(
            "snapshot file missing for '{fixture_label}': {}\n\
             Run `UPDATE_SNAPSHOTS=1 cargo test snapshot_synthesised` to generate it.\n\
             Error: {e}",
            snap_path.display()
        )
    });

    // Normalise: strip a single trailing newline that may have been added on
    // write so the comparison doesn't care about trailing-newline presence.
    let stored = stored.trim_end_matches('\n');
    let actual_norm = actual.trim_end_matches('\n');

    if stored != actual_norm {
        // Build a simple unified-style diff for the failure message.
        let stored_lines: Vec<&str> = if stored.is_empty() {
            vec![]
        } else {
            stored.lines().collect()
        };
        let actual_lines: Vec<&str> = if actual_norm.is_empty() {
            vec![]
        } else {
            actual_norm.lines().collect()
        };

        let mut diff_lines: Vec<String> = Vec::new();
        // Lines only in stored snapshot.
        for line in &stored_lines {
            if !actual_lines.contains(line) {
                diff_lines.push(format!("- {line}"));
            }
        }
        // Lines only in current output.
        for line in &actual_lines {
            if !stored_lines.contains(line) {
                diff_lines.push(format!("+ {line}"));
            }
        }

        panic!(
            "synthesised-posting snapshot mismatch for '{fixture_label}'.\n\
             Snapshot: {}\n\
             \n\
             Diff (- = stored only, + = current only):\n\
             {}\n\
             \n\
             To regenerate: UPDATE_SNAPSHOTS=1 cargo test snapshot_synthesised",
            snap_path.display(),
            diff_lines.join("\n"),
        );
    }
}

// ---------------------------------------------------------------------------
// One test function per positive-parity fixture (mirrors scripts/parity_check.py
// POSITIVE list). Labels use the same `frontend:name` convention.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_beancount_sample() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_sample",
        &root.join("crates/doppio/tests/fixtures/sample.beancount"),
    );
}

#[test]
fn snapshot_hledger_sample() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_sample",
        &root.join("crates/doppio-cli/tests/fixtures/sample.hledger"),
    );
}

#[test]
fn snapshot_ledger_sample() {
    let root = workspace_root();
    assert_snapshot("ledger_sample", &root.join("web/fixtures/sample.ledger"));
}

#[test]
fn snapshot_ledger_transfer() {
    let root = workspace_root();
    assert_snapshot(
        "ledger_transfer",
        &root.join("tests/parity/ledger-transfer.ledger"),
    );
}

#[test]
fn snapshot_ledger_demo() {
    let root = workspace_root();
    assert_snapshot("ledger_demo", &root.join("tests/parity/ledger-demo.ledger"));
}

#[test]
fn snapshot_ledger_no_trailing_newline() {
    let root = workspace_root();
    assert_snapshot(
        "ledger_no_trailing_newline",
        &root.join("tests/parity/ledger-no-trailing-newline.dat"),
    );
}

#[test]
fn snapshot_ledger_drewr3() {
    let root = workspace_root();
    assert_snapshot(
        "ledger_drewr3",
        &root.join("tests/parity/ledger-drewr3.dat"),
    );
}

#[test]
fn snapshot_hledger_quickstart() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_quickstart",
        &root.join("tests/parity/hledger-quickstart.journal"),
    );
}

#[test]
fn snapshot_hledger_ascii() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_ascii",
        &root.join("tests/parity/hledger-ascii.journal"),
    );
}

#[test]
fn snapshot_hledger_zerostar() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_zerostar",
        &root.join("tests/parity/hledger-zerostar.journal"),
    );
}

#[test]
fn snapshot_hledger_zerostar_subtree() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_zerostar_subtree",
        &root.join("tests/parity/hledger-zerostar-subtree.journal"),
    );
}

#[test]
fn snapshot_hledger_block_comment() {
    let root = workspace_root();
    assert_snapshot(
        "hledger_block_comment",
        &root.join("tests/parity/hledger-block-comment.journal"),
    );
}

#[test]
fn snapshot_beancount_example() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_example",
        &root.join("tests/parity/bean-example.beancount"),
    );
}

#[test]
fn snapshot_beancount_subtree_balance() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_subtree_balance",
        &root.join("tests/parity/beancount-subtree-balance.beancount"),
    );
}

#[test]
fn snapshot_beancount_subtree_pad() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_subtree_pad",
        &root.join("tests/parity/beancount-subtree-pad.beancount"),
    );
}

#[test]
fn snapshot_beancount_starter() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_starter",
        &root.join("tests/parity/beancount-starter.beancount"),
    );
}

#[test]
fn snapshot_beancount_basic() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_basic",
        &root.join("tests/parity/beancount-basic.beancount"),
    );
}

#[test]
fn snapshot_beancount_fifo() {
    let root = workspace_root();
    assert_snapshot(
        "beancount_fifo",
        &root.join("tests/parity/beancount-fifo.beancount"),
    );
}

#[test]
fn snapshot_ledger_wow() {
    let root = workspace_root();
    assert_snapshot("ledger_wow", &root.join("tests/parity/ledger-wow.dat"));
}

#[test]
fn snapshot_ledger_standard() {
    let root = workspace_root();
    assert_snapshot(
        "ledger_standard",
        &root.join("tests/parity/ledger-standard.dat"),
    );
}
