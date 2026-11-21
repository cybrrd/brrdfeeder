<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

# Wire v5 P0 acceptance registration

Registered 2026-11-16, before implementation. This document is the first commit
on `feat/wire-v5-p0`. It specifies future acceptance; it does not claim P0 passes.

Engine baseline: `cybrrd/brrdfeeder` main
`9faae55f53e27cea198401ee34b4c66c1898666a`.
Design baseline: the ratified internal wire v5 design baseline (its contract,
semantics, encoding, maps and fixtures are listed below).
The 2026-11-16 P0 dispatch authorizes this development branch; the design alone
does not authorize implementation or deployment. The current checkpoint ends
after registration and reporting, before implementation.

## Contract and scope

The acceptance authority is `ACCEPTANCE.md` together with `SEMANTICS.md`,
`ENCODING.md`, `cbor-keys.json`, `observation.schema.json`, both inventory maps,
`golden-messages.json`, `golden-observations.json`, and
`semantic-negative-cases.json` at the design baseline. The maps contain 136
original homes (57 message/layout, 44 transport, 35 receiver) and four additional
network homes. Existing seeds comprise 175 message vectors and six observation
fixtures. The 195 semantic-negative records include later-phase cases and are
an inventory of rejection expectations, not an executable P0 test suite.

P0 adds an isolated complete message/observation model, a raw capture ledger
interface ahead of lossy processing, and version-aware test ingress. All model
homes remain representable, including fields whose live adapters arrive in P1.
No received identifier, position, header, reserved bit, repeated message or
unknown byte may be discarded, normalized away, masked or replaced by a hash.
Malformed and unsupported input remains exact evidence with a diagnosis.

P0 evidence covers capture-interface input, extraction/model, canonical encoding,
decode and test ingress. Production receiver wiring, clocks/signatures, durable
WAL and custody, NATS subjects, lake columns, globe and deployment remain P1/P2.
Test ledger receipts prove interface ordering, not fsync or archive survival.
No incomplete development record may masquerade as production-complete v5.

## P0 golden families and falsification criteria

| ID | Required P0 cases and assertions | Deliberate failure that must be detected |
|---|---|---|
| P0-01 Messages and multiplicity | Types 0–5 and reserved/unknown types; all 16 version nibbles on messages and packs; counts 0, 1, 9, 10 and malformed size/count combinations; all 16 ID types and UA type codes; equal and conflicting repeated Basic IDs. Preserve header, raw bytes, ordinals, trailing bytes and every Basic ID index/body link. Only type 1 is serial. | Drop/reorder/duplicate a message, select the first ID, reinterpret a nonserial ID as serial, change an index or decode an unsupported version as v2. Each changes the independent expected result and fails. Private version 15 retains private-use header status and unsupported parse status without a registered decoder. |
| P0-02 No-position observations | Auth-only, System-only, Self-ID-only and Operator-ID-only inputs, without Basic ID or aircraft location, reach the ledger/model/codec/test-ingress output. Missing messages remain absent. | Reintroduce an ID/coordinate admission gate, suppress a record, or invent a position. Compare input/output observation identities and full bytes; any loss/default fails. Production audit/archive publication remains later-phase evidence. |
| P0-03 Authentication | Pages 0–15, continuation first then page 0, missing/duplicate/conflicting pages, counter wrap and reuse, payload lengths 0/255, additional-data lengths zero/nonzero/truncated, exact padding, all 16 Auth types and method ranges. Preserve every page/reference; use the low nibble for last page and preserve its reserved high nibble. All eight cells exist with opposite-page N/A. Exercise reconstruction of payload/additional data/padding and ambiguity from retained pages in the test harness. | Whole-byte last-page decoding, first-page-wins, loss of a continuation or duplicate, combining conflicting/wrapped generations, truncating additional data, or declaring authentication verified must fail. Type 0 empty/nonempty differs from type 4 required-null/nonzero; type 3 transport violations remain issues. Source observations never depend on successful assembly. |
| P0-04 Numeric domains | Exhaust all 256 signed vertical codes; 512 horizontal byte/multiplier pairs; 512 direction byte/segment pairs; 65,536 codes in each of six altitude/height fields (393,216 field cases); both height types; all 65,536 Location timestamp codes; all 16 codes of each accuracy nibble. Compare raw, exact decimal arithmetic, status, unit, resolution and reason with an independent oracle. Auth/System u32 timestamps require a full-domain arithmetic proof plus executable boundary/byte-position cases; finite sampling alone does not satisfy the all-codes requirement. | Vertical raw 126 becoming known or raw 63 becoming unknown; saturation clamping; altitude raw 0 conflated with raw 2000; sentinel converted to zero; wrong timestamp epoch/range; reserved accuracy converted to exact zero error. Each must fail independently. |
| P0-05 Datum, endpoints and remaining semantics | All six altitude homes: pressure, geodetic HAE, height, area ceiling HAE, area floor HAE, operator HAE. Raw 65535 requires decoded and arithmetic_value `"31767.5"`, status `out_of_stated_range`, reason `arithmetic_exceeds_stated_range`. Cover coordinate limits, joint-zero versus single-zero, System source/area/classification/category/class, and reserved bits/codes. | For each altitude home, seed null/clamped/wrong arithmetic, known/ambiguous status, missing reason or changed raw. Swap HAE to MSL, merge height/pressure into geodetic altitude, or erase classification inconsistency; all must fail. Preserve the pressure-reference conflict without an invented conversion. |
| P0-06 Empty/NUL/applicability | Empty and all-NUL IDs/descriptions, leading/embedded NUL with nonzero tail, spaces and whitespace-only text, invalid ASCII, binary UUID/session zero octets, type 0 undeclared bytes and all reserved ID types. Every opposite Auth/transport group has N/A/null/reason; active unexposed hardware fields use not_exposed. All four network cells are N/A on broadcast. | Trim spaces, remove tail/padding, mark empty text known, treat UUID zero bytes as terminators, omit an N/A cell, populate N/A, mark active data N/A, or substitute zero/empty/default network data. Exact-byte and semantic checks must fail. |
| P0-07 Malformed and unsupported | Empty input; every truncation boundary for each fixture; corrupt lengths/counts, bad FCS evidence, invalid text, unknown versions, oversized/fractured test parcels, duplicate JSON/CBOR keys, unknown/misplaced tags. Retain the exact input with a bounded-work diagnostic. Ledger append precedes parsing/filter/dedup; rejected append prevents an acceptance result. | Pad missing bytes, discard parse failures, silently acknowledge an unretained parcel, or claim malformed/unsupported data conforms. Inject append failure and parser rejection separately; a success receipt without retained input fails. Durable quarantine is a P1 gate. |
| P0-08 Transport/model coverage | Round-trip all six supplied observation fixtures (four principal transports plus separate BT5-primary and NAN-sync companions). Verify all 140 homes, raw capture/unit/message coverage, counters, source identities, separate capture IDs and relationship order. Add independently checked synthetic boundary fixtures for the adapter formats exercised in P0, declaring every exposure gap. | Strip a header/counter/trailing byte, fold NAN sync into SDF, fabricate a missing BT primary, lose raw frame equality or call HCI data a full RF packet. An uncovered input byte or missing home fails. Hardware and optional sniffer-tier conformance cannot be inferred from these fixtures. |
| P0-09 JSON/CBOR/evolution | Every message expected record and every complete observation golden, including new P0 vectors, passes canonical JSON ↔ CBOR ↔ JSON using dictionary revision 0 and schema-typed byte tag 60000. Preserve extensions, exact original parcels, Unicode, array order and absent/null/empty/zero/false distinctions. Independently implemented decoding checks canonical bytes and logical values. Ingress explicitly distinguishes v4, historic unversioned, v5 and unknown major versions. | Lose an optional member, coerce a decimal/counter to float, reorder an array, accept a dictionary text alias, unknown integer key without its dictionary, duplicate key, untagged bytes or misplaced tag. Unsupported originals must survive; claiming complete interpretation fails. |
| P0-10 Mutations | For every instantiated golden field and mapped home, delete it once and alter it once independently at each P0 boundary. Cover containers, list elements, ordinals, repeats, extensions, statuses, units, reasons and raw bytes. Populate optional homes in supplemental synthetic cases. Publish field × boundary denominators and first-failing-boundary evidence. | Any single lost/altered field that escapes the comparator fails acceptance. Include counter mutation, changed-content same-ID input, ID reinterpretation and message/page reorder. Reinject the six historic defects: serial fallback, HAE/MSL swap, whole-byte last page, continuation-first loss, changed-content dedup and unchecked version. Demonstrate red, restore, demonstrate green. |
| P0-11 v4 regression | Keep v4 wire behavior, existing golden sources and 0.9.0 paths unchanged. Run the existing core/protocol suites including altitude_v4 and position_v4 plus affected engine regression tests before a draft implementation PR. Compare the diff against the pinned base. | Any changed v4 output, altered legacy expectation, skipped required test or zero-test run fails its gate. A v5 improvement cannot justify changing a v4 fallback or sentinel. |

## Evidence rules and exit gate

Expected extraction/arithmetic must not be generated by the production decoder
under test. Round-trip equality alone can hide a shared codec defect. Pin oracle,
fixture, schema, dictionary and engine revisions in the receipt; record commands,
exit codes, executed/passed/failed/skipped counts and exhaustive domain sizes.
Report proof coverage separately from executable case counts, especially for
u32 timestamps. If the full-domain timestamp proof cannot be independently
checked, that requirement stays unfulfilled; do not silently substitute samples.

Before implementation exit, classify every one of the 195 supplied negative
expectations as a P0 executed case or a named P1/P2 deferral. Browser display,
named lake columns and production-hop controls are deferred, never counted as
P0 passes. All model/codec negatives applicable to P0 must execute. Likewise,
transport evidence states its exact adapter boundary and exposure denominator.

P0 must show complete raw coverage and independent message/transport golden
results, no-location observations, counter/ID/order mutations, unchanged v4
regressions and all registered P0 cases satisfied. The implementer cannot review
their own work. A review by a different model is required at implementation
exit, and the independent auditor audits regardless of author. This registration checkpoint does
not claim that review has occurred. Draft PR only; never merge or deploy.

## Corpus dependency register

This is an engineering dependency register, not a corpus edit or a quotation
of licensed material. Record the eventual implementation file:line and test for
each dependency as code is added. Keep licensed source citations in local notes.

| Corpus items | P0 dependency / treatment |
|---|---|
| C01 | System source applies to both operator location and altitude; test both. |
| O01 | Area ceiling/floor retain HAE datum. |
| O02 | Complete BT5 transport homes and raw compound values; adapter-dependent exposure remains explicit. |
| O07 | Accuracy is a bound code, not a verified exact error or certification claim. |
| O12 | Complete information homes and four broadcast N/A network fields; no Network RID service implementation. |
| O17 | Private protocol version is separate from message type. |
| I01 | Preserve pressure-reference discrepancy; no silent conversion. |
| I02, I03 | Preserve actual AuxPtr/access-address bytes under a named adapter profile; do not pad or silently settle source layout/unit conflicts. |
| I04 | Preserve Auth raw timestamp and epoch arithmetic despite printed end-year discrepancy. |
| I06 | Preserve method codes/private/reserved evidence without asserting compliance. |
| Separate vertical-speed correction and altitude endpoint issue | Use the design's settled vertical domain and ratified altitude Option B; source contradiction remains explicit. |
| C02–C05; O03–O06, O08–O11, O13–O16; I05 | No P0 service dependency presently identified. Network service, verifier, reporting and editorial corrections remain separate; reassess if P0 touches them. |

Newly discovered dependencies or contradictions must be reported before changing
the accepted semantics. Corpus correction precedes production generation; P0 is
a development build and does not settle ASTM contradictions.

## Boundaries, alternatives and baseline receipt

Use an additive v5 path: modifying the legacy projection would violate Decision
6. Reconsider shared runtime integration only in the authorized later phases with
unchanged v4 proof. A generic JSON round-trip alone is insufficient; retain typed
field/raw-byte assertions and independent expected values. Synthetic boundary
fixtures cannot replace hardware evidence; reconsider that claim only when the
named hardware tier has an independently checked receipt.

No dissent from complete-data retention or the registered scope. No certification,
production durability, hardware coverage or rollout-readiness claim is made.
Reviewer/merge priority remains with the existing release work; the packet names
0.9.1, while the ratified Decision 6 names 0.9.0 and its December 5 checkpoint.
Neither priority is displaced by P0.

Before this registration commit, the unchanged command
`cargo test --locked --manifest-path Component/aviary/Cargo.toml -p cybrrd-rid-core -p cybrrd-rid-protocol`
executed 70 tests: core 16/16, protocol unit 32/32, altitude_v4 5/5,
position_v4 6/6, wifi_nan 11/11; none failed, ignored or filtered.
The two empty doc-test targets provide no coverage and are not counted as passes.
An existing unused-test-helper warning remains. Engine tests were not run at
this documentation-only checkpoint. P0 implementation tests executed: 0;
P0 golden families passed: 0/11. These are pending, not successful zero-test runs.

## Post-registration edit

2026-11-16, after implementation: the design-baseline line near the top was
reworded to remove an internal repository name, commit and path. No acceptance
criterion, gate, denominator or scope changed. The original wording is in this
branch's first commit.
