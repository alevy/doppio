//! Wire-format evolution discipline checks for `proto/doppio.proto`.
//!
//! Two invariants every PR must preserve:
//!
//! 1. **No tag reuse**: a tag number may appear at most once per
//!    message -- either as a live field or in a `reserved` clause, not
//!    both, and never twice as a field.
//! 2. **No silent gaps**: if a message's live field tags are
//!    `{1, 2, 5}`, then `3` and `4` must appear in a `reserved` clause
//!    (or there must be a documented reason in the proto comment why
//!    they were skipped).
//!
//! Together these enforce the "additive only + reserved on deprecation"
//! rule from `docs/proto-evolution.md` mechanically. A PR that removes
//! a field without reserving its tag fails this test; a PR that
//! accidentally reuses a tag also fails.
//!
//! The parser here is regex-based and intentionally narrow -- it
//! understands the subset of proto syntax that `doppio.proto` actually
//! uses (top-level `message Foo { ... }` blocks; field declarations of
//! the form `[repeated|optional]? <type> <name> = <tag>;` plus
//! `map<K, V>` and inline message variants; `reserved <tag>[, <tag>]*;`
//! and `reserved "<name>"[, "<name>"]*;`). Adding wider syntax (e.g.
//! `oneof` or `extend`) would mean extending the scanner.

use std::path::PathBuf;

const PROTO_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proto/doppio.proto");

#[derive(Debug, Default)]
struct Message {
    name: String,
    field_tags: Vec<u32>,
    reserved_tags: Vec<u32>,
}

fn parse_proto(src: &str) -> Vec<Message> {
    let mut out = Vec::new();
    let mut current: Option<Message> = None;
    let mut depth = 0; // brace depth INSIDE current message body

    for raw_line in src.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = parse_message_header(line) {
            // Top-level `message X {` -- start a new message scope.
            assert!(
                current.is_none(),
                "nested top-level message? at: {raw_line}",
            );
            current = Some(Message {
                name,
                ..Default::default()
            });
            depth = 1; // the brace that opens the body
            continue;
        }

        let Some(msg) = current.as_mut() else {
            continue;
        };

        // Track brace depth so nested `oneof { ... }` or option blocks
        // (none in doppio.proto today, but safer than ignoring) close
        // correctly.
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth <= 0 {
            // The closing brace of the message body.
            out.push(current.take().unwrap());
            depth = 0;
            continue;
        }

        if let Some(tag) = parse_field_tag(line) {
            msg.field_tags.push(tag);
        } else if let Some(tags) = parse_reserved_tags(line) {
            msg.reserved_tags.extend(tags);
        }
    }

    assert!(current.is_none(), "unterminated message in proto file");
    out
}

fn strip_comment(line: &str) -> &str {
    line.split_once("//")
        .map(|(before, _)| before)
        .unwrap_or(line)
}

fn parse_message_header(line: &str) -> Option<String> {
    // Accept `message Foo {` on a single line.
    let rest = line.strip_prefix("message ")?.trim();
    let (name, rest) = rest.split_once(|c: char| c == '{' || c.is_whitespace())?;
    if !rest.trim_start().starts_with('{') && !rest.is_empty() {
        // `message Foo extends ...` not used here; tolerate with a
        // best-effort name extraction.
    }
    Some(name.trim().to_string())
}

/// Extract a tag number from a field declaration line.
///
/// Recognises proto3 field forms:
///   - `Type name = N;`
///   - `repeated Type name = N;`
///   - `optional Type name = N;`
///   - `map<K, V> name = N;`
///
/// Skips `reserved` and `option` lines (handled separately).
fn parse_field_tag(line: &str) -> Option<u32> {
    // Reject keyword lines. Word-boundary aware so `optional` (the
    // proto3 modifier) is not mistaken for `option` (the file/message
    // option keyword).
    let first_word = line.split(|c: char| c.is_whitespace() || c == '{').next()?;
    if matches!(first_word, "reserved" | "option" | "oneof" | "enum") {
        return None;
    }
    // Look for `= <digits>;` as the field-tag suffix.
    let semi = line.rfind(';')?;
    let before_semi = &line[..semi];
    let eq = before_semi.rfind('=')?;
    let after_eq = before_semi[eq + 1..].trim();
    after_eq.parse::<u32>().ok()
}

/// Parse `reserved 3, 4, 5;` style clauses. Tags as strings (`reserved "foo";`)
/// are NOT parsed here -- name reservations don't constrain tag numbers.
fn parse_reserved_tags(line: &str) -> Option<Vec<u32>> {
    let rest = line.strip_prefix("reserved")?.trim_start();
    let body = rest.trim_end_matches(';').trim();
    let mut tags = Vec::new();
    for piece in body.split(',') {
        let p = piece.trim();
        if p.is_empty() || p.starts_with('"') {
            // Skip string-name reservations.
            continue;
        }
        if let Some((lo, hi)) = p.split_once("to") {
            // `reserved 3 to 5` range form.
            let lo: u32 = lo.trim().parse().ok()?;
            let hi: u32 = if hi.trim() == "max" {
                return None; // open-ended; not used in doppio.proto
            } else {
                hi.trim().parse().ok()?
            };
            tags.extend(lo..=hi);
        } else if let Ok(t) = p.parse::<u32>() {
            tags.push(t);
        }
    }
    Some(tags)
}

#[test]
fn proto_file_parses() {
    let src = std::fs::read_to_string(PathBuf::from(PROTO_PATH)).expect("read proto");
    let messages = parse_proto(&src);
    assert!(
        !messages.is_empty(),
        "expected at least one message in proto/doppio.proto, found none — scanner likely broke",
    );
    // Spot check: known top-level messages should be present.
    let names: Vec<&str> = messages.iter().map(|m| m.name.as_str()).collect();
    for expected in ["Decimal", "Amount", "Posting", "Transaction", "Journal"] {
        assert!(
            names.contains(&expected),
            "expected to find message {expected:?} in proto, only saw {names:?}",
        );
    }
}

#[test]
fn no_duplicate_tags_within_a_message() {
    let src = std::fs::read_to_string(PathBuf::from(PROTO_PATH)).expect("read proto");
    for msg in parse_proto(&src) {
        let mut seen = std::collections::BTreeSet::new();
        for tag in &msg.field_tags {
            assert!(
                seen.insert(*tag),
                "tag {tag} declared twice as a field in message {}",
                msg.name,
            );
        }
        for tag in &msg.reserved_tags {
            assert!(
                !msg.field_tags.contains(tag),
                "tag {tag} appears as both a live field AND `reserved` in message {} \
                 — pick one",
                msg.name,
            );
        }
    }
}

#[test]
fn no_silent_gaps_in_field_tags() {
    let src = std::fs::read_to_string(PathBuf::from(PROTO_PATH)).expect("read proto");
    for msg in parse_proto(&src) {
        let Some(&max) = msg.field_tags.iter().max() else {
            continue;
        };
        let live: std::collections::BTreeSet<u32> = msg.field_tags.iter().copied().collect();
        let reserved: std::collections::BTreeSet<u32> = msg.reserved_tags.iter().copied().collect();
        for tag in 1..=max {
            assert!(
                live.contains(&tag) || reserved.contains(&tag),
                "tag {tag} is missing in message {} — gaps below the highest live tag \
                 must appear in a `reserved` clause (proto-evolution policy: see \
                 docs/proto-evolution.md)",
                msg.name,
            );
        }
    }
}
