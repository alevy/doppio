# `.dop` wire-format evolution policy

**Status**: normative from doppio 1.0 forward.
**Audience**: doppio maintainers; downstream consumers writing their own
`.dop` readers/writers; anyone reviewing changes to `proto/doppio.proto`
or to the `.dop` header.

doppio's value proposition includes a stable, language-agnostic
compiled-journal format. Consumers in any language can read `.dop` files
via the published `proto/doppio.proto` schema. That promise only holds
if the format evolves under documented rules -- and these are those rules.

## Two layers, two cadences

The `.dop` artifact has two evolvable layers:

1. **The 8-byte file header**: magic + format version (u16 LE) +
   compression byte + reserved byte. The format version is the *header
   version*; readers reject files whose version they don't support.
2. **The protobuf body**: serialised `proto::Journal`. Evolution under
   protobuf's [field-presence and forward-compatibility rules](https://protobuf.dev/programming-guides/proto3/).

The two layers evolve independently. Most additions happen in the
protobuf body without touching the header version.

## When does the header version bump?

The format version (`DOP_FORMAT_VERSION`, currently `3`) bumps **only**
when a change is **breaking for old readers**. Breaking means: a reader
built against the old version produces wrong output (or crashes) on a
file written by the new version.

Bump-required examples:

- Changing the wire encoding of an existing field type (e.g. switching
  `Decimal`'s mantissa from split-`uint64`/`sint64` to a `bytes` field).
- Removing a `proto::Journal` top-level field that old readers
  *required* (e.g. they assumed `transactions` is always populated).
- Changing the meaning of an existing field number (e.g. repurposing
  `Decimal.scale` from "decimal places" to something else).
- Adding a new compression byte value (old readers reject the unknown
  byte rather than silently mishandle the body).

**Not** bump-required:

- Adding a new optional field to any message (proto3 default for
  message fields). Old readers ignore unknown fields.
- Adding a new message type that's only referenced by new fields.
- Adding a new enum variant (proto3 enums are open: unknown variants
  decode to the underlying integer).
- Removing a field whose absence is benign for old readers (rare; in
  practice, most field removals are breaking).

When the version bumps, doppio publishes both the new format version
and the rationale in `CHANGELOG.md`. Readers from the previous major
release continue to support the old version for at least one minor
release before being retired.

## Protobuf body evolution rules (1.0+)

These rules apply to every `.proto` file change:

### 1. Additive only

New fields get new tag numbers. Existing tag numbers never change
meaning. Existing field names never change meaning.

If you want to rename a field, do so without changing its tag number;
the wire encoding is unaffected. If you want to repurpose a field,
**don't** -- add a new field with a new tag and deprecate the old one.

### 2. Reserved on deprecation

When a field is removed (or its semantics genuinely change in an
incompatible way that requires a new field), its tag number AND its
field name are added to the message's `reserved` clause. Reserved tags
and names can never be reused. Example:

```proto
message Foo {
  // OLD (pre-removal):
  // string old_name = 3;

  // NEW (after removal):
  reserved 3;
  reserved "old_name";

  string new_name = 5;  // 4 was also reserved if needed
}
```

This is enforced by a CI check (`crates/doppio/tests/proto_evolution.rs`)
that fails if a tag number is skipped without a `reserved` clause
covering it.

### 3. Map and oneof fields are first-class

Adding a `map<K, V>` field follows the same rules as any other field:
new tag, additive, reserved on removal. Same for `oneof` groups.

Note: `map` field iteration order is **not specified** by the proto
spec. doppio's Rust binding configures prost to emit `BTreeMap` for
deterministic ordering, but consumers in other languages must sort by
key explicitly when reproducibility matters. See `proto/doppio.proto`'s
top-comment recipes for examples.

### 4. Behaviour preservation

For a fixed input source file and fixed compiler version, `dop compile`
must produce **byte-identical** `.dop` output across patch and minor
releases -- modulo documented format-version bumps. This is what
downstream consumers rely on for diff-based reproducibility (e.g.
"did anything change?" CI checks on committed `.dop` files).

The Rust binding uses `BTreeMap` for every map field for exactly this
reason. Adding a new field to a message changes the wire bytes (a new
tag appears), which is by-construction expected and not a regression.

## How a typical change lands

1. Edit `proto/doppio.proto` -- add the new field with a fresh tag
   number, document its semantics in a comment.
2. Update `crates/doppio/src/elaborator.rs` (or wherever the proto type
   is constructed) to populate the new field.
3. Update consumers -- Rust callers via the regenerated prost types,
   JS callers via the regenerated Buf types in `web/dashboard/src/lib/proto/`.
4. Add a parity fixture or unit test that exercises the new field.
5. CHANGELOG entry under the next version's "Added" section.

For a removal:

1. Add the tag and name to `reserved` in the same PR that deletes the
   field.
2. If old readers depended on the field, bump `DOP_FORMAT_VERSION` and
   document the migration path.
3. CHANGELOG entry under "Breaking changes" if applicable.

## Beyond doppio's own consumers

Third parties writing readers in other languages (e.g. the JS-native
`.dop` reader in `web/dashboard/src/lib/dop/`) follow the same evolution rules
when they project the proto types into idiomatic shapes. The proto
schema is the source of truth; language ports adapt to it, not the
other way around.

## Reference

- Protobuf official guide: [proto3 evolution rules](https://protobuf.dev/programming-guides/proto3/#updating)
- doppio header layout: see `proto/doppio.proto`'s top comment block
  and `crates/doppio/src/lib.rs::dop_write_header`.
- The CI check enforcing the reserved-tag discipline:
  `crates/doppio/tests/proto_evolution.rs`.
