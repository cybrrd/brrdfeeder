<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# YAML configuration semantics in 0.9.1

The engine switches from serde_yaml 0.9.34 to serde-saphyr 1.1.0.
These choices apply to typed EngineConfig loading, including validation, rather
than only to a generic YAML value parser. Hand-edited configurations may need
the changes below before upgrading.

| Input | Previous parser | Chosen engine behavior |
| --- | --- | --- |
| `.nan`, `.NaN`, `.inf`, `-.inf` | Could deserialize non-finite floats; engine location validation still refused them | Refuse non-finite coordinates at load. Never substitute zero or a default. |
| Unquoted `yes/no/on/off` | Generic values are strings; bool fields refused these words | Bool destinations accept yes/on true, no/off false, including `sensors.rid_ble.unblock_rfkill`. A typed `node.id` retains the original word as a string, quoted or unquoted. |
| Unquoted `011` | Generic value is a string | Numeric latitude is decimal 11, never octal 9. A typed string ID remains exactly `011`, quoted or unquoted. |
| Unquoted `1_000` | Generic value is a string | Numeric heartbeat interval is 1000. A typed string ID remains exactly `1_000`, quoted or unquoted. |
| `<<:` merge key | Retained as a literal key (and refused by typed structs with unknown-field checks) | Expand merges; an explicit field overrides the merged value. Unknown merged fields remain errors in structs with `deny_unknown_fields` (tested on node); incorrectly typed known fields remain errors. |
| Literal block `|` | Review differential observed trailing-newline stripping | Preserve the newline. Use `|-` to strip it explicitly. No implicit trimming of strings. |

Safety decision: retain the parser's documented scalar and merge behavior, with
typed schema checks and existing value validation. Boolean words express the
same true/false intent; numeric forms have explicit decimal meaning. Merges
respect the destination schema, and explicit settings take precedence.
Non-finite coordinates are rejected for safety; string IDs keep their exact
spelling rather than being normalized to boolean/numeric text. We do not
add a lexical rewrite layer that could alter quoted strings, anchors or blocks.
For critical hand-written paths, IDs and capability settings, prefer quoted
strings and canonical lowercase `true`/`false`; inspect merges before deployment.
Block scalars are usually inappropriate for single-line IDs or paths: a retained
newline is part of the value, not whitespace the engine silently removes.

The review's generic-value differential is not equivalent to typed loading:
the suggested `node.id: yes` refusal does not occur with a String destination.
The regression tests explicitly assert the observed byte-preserving behavior
instead of adding an incompatible ID rejection policy. The custom
`node.status_file` deserializer uses a generic value first and still rejects
non-string values. Also, GPS currently lacks `deny_unknown_fields`; this delta
does not claim unknown-field refusal there or change that pre-existing policy.

Installer output is unchanged: the six compatibility fixtures retain the exact
old-parser full typed snapshots (including defaults and loaded-byte hashes).
The template/helper-write coverage test checks all installer-owned fields, so
future writes cannot silently escape the fixture set. No installer bytes or
pins change in this delta. Tests in `node_config::yaml_migration_tests` pin each
choice through EngineConfig::load, including quoted escape hatches, merge
override/error behavior and both literal-block chomping forms.
