# Wire v5 phase 0 — offline development decoder

This crate decodes complete RID messages and synthetic observations without connecting to a radio, engine, publisher or network service. It has no runtime dependency on the v4 decoder. Its only workspace changes are membership and a new lockfile package record; existing dependency versions remain unchanged.

`CaptureRecord` carries exact capture bytes, capture identity, receive metadata, boundary provenance and explicit RID-unit spans. `decode_capture` consumes it and returns the original record on every path, alongside an observation and diagnostics. A malformed span, unsupported version or parser-budget failure cannot erase the original bytes. Capture length and unit-count budgets bound decoding; collecting the input itself remains the caller's responsibility. This is an in-memory interface, with no WAL, fsync or custody guarantee.

Messages retain raw headers and bytes, all ordered/repeated Basic IDs, typed ID codes, distinct altitude datums, System classification and every Authentication page. Unknown message types and versions are raw-only. Complete extraction does not imply conformance. Diagnostic records are returned separately from immutable observation cells; no output asserts cryptographic verification. Authentication assembly is a separate derived revision and retains source pages, duplicates and conflicts. Cross-capture grouping requires explicit association evidence in addition to identity, transport and counter agreement.

The model follows the pinned schemas in `contract/`. Field cells retain raw/decoded values, status, unit, resolution and reason; altitude cells also retain arithmetic values. Measurements use exact decimal strings. Raw altitude `65535` produces `"31767.5"`, `out_of_stated_range`, and `arithmetic_exceeds_stated_range` for all six altitude/height fields. Broadcast-inapplicable network fields remain explicit N/A.

The canonical codec preserves unknown additive members, array order, null/empty distinctions, and exact original parcels. It rejects unsafe integers, floats, duplicate keys, unsupported CBOR tags, invalid tag placement and noncanonical CBOR forms. A parsed parcel's profile label is a version-routing result, not schema validation or a conformance verdict. Structural and semantic validation is exercised independently by the Python contract gate.

Run from the repository root with Rust 1.88 and the locked dependencies already cached:

```sh
cargo test --offline --locked --manifest-path Component/aviary/Cargo.toml -p cybrrd-rid-v5
python3 Component/aviary/cybrrd-rid-v5/tools/check.py --out /tmp/v5-default-receipts
python3 Component/aviary/cybrrd-rid-v5/tools/release_gate.py --out /tmp/v5-exhaustive-receipts
```

The Python contract gate requires the version of `jsonschema` pinned in `tools/requirements-test.txt`. `check.py` runs formatting, Clippy, the default tests, a release build, and an independent Python/schema/CBOR comparison. It writes the mutation matrix and named regression examples. The normal test suite uses deterministic representative bindings for large numeric domains. Three exhaustive Rust tests are ignored in the default suite and run only through the separate release target.

`release_gate.py` records contiguous u32 range receipts with the tested binary hash, then runs the smaller exhaustive domains. It checks 8,590,394,704 numeric field/code cases, 33,554,986 reserved patterns, and 66,904 other semantic cases. The u32 sweep exercises numeric primitives used by both timestamp fields; default tests separately check full-message offsets and endianness. All six u16 altitude bindings are exercised through the full message decoder. The 24-bit reserved sweep tests the exact byte codec across the full domain, with deterministic full-message binding checks. These boundaries are recorded explicitly and are not hardware or production-hop proof.

The mutation matrix covers 140 inventory rows, their alias homes, all 5,004 scalar leaves in the six pinned observations, richer synthetic metadata, and derived assembly output. Every declared leaf receives deletion and value-corruption controls. The two polygon-coordinate leaves under broadcast-inapplicable network data are exercised as schema/codec alternatives, not claimed as broadcast decode paths. The 195 pinned negative controls include display/lake contract checks; no browser or lake implementation is exercised in this phase.

The offline ingress binary accepts one JSON request on stdin. Examples:

```sh
printf '%s' '{"mode":"message","hex":"02000000000000000000000000000000000000000000000000"}' |
  cargo run --offline --locked --manifest-path Component/aviary/Cargo.toml -p cybrrd-rid-v5 --bin v5-test-ingress
```

Modes are `message`, `unit`, `capture`, `fixture`, `assemble`, `codec`, `decode_cbor`, `parcel`, and `verify`. `capture` takes a serialized `CaptureRecord` whose `bytes` field is lowercase hex. `fixture` derives units from one synthetic contract observation but never reads its expected decoded message bodies. `parcel` retains exact JSON/CBOR input and routes explicit v4, v5, historic unversioned and unknown-major inputs without rewriting history. `verify` compares an independently supplied expected tree and reports the first differing JSON pointer; equality alone is not a semantic oracle.

All committed vectors are synthetic. Contract provenance and exact hashes are in `contract/PROVENANCE.json`. Hardware capture adapters, durable custody, ingest, subscriber/lake integration, frontend presentation and deployment remain later-phase work. Existing repository CI continues running its existing contracts; these new gates have explicit commands and receipts and do not claim automatic inclusion in that workflow.
