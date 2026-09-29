# requirements/ — the governed-claims ledger (Executable Contracts)

Doctrine: pack-stack `docs/metasystem/executable-contracts.md` (Purple proposal, 2026-07-13).

> **The rule:** no claimed capability without a single-sourced, executable contract that CI
> can refute.

- One YAML file per requirement (Doorstop-shaped records; EARS statement syntax —
  `WHEN <trigger>, the <system> SHALL <response>` / `WHILE <state> …` / `IF <cond>, THEN …`).
- **These files are the truth.** Any Requirement-Track instance (BASIL) is a rebuildable
  projection of them — dashboards, coverage, test-run evidence, SPDX design-SBOM export.
- Only **governed** behaviors live here (economy clause): safety bounds, wire formats,
  trust/custody paths, config surface, install/uninstall, radio conduct — the set a
  Sentinel watches and a Daubert challenge would probe. Ordinary code needs tests and
  doc-comments, not a REQ record.

**Fields:** `id` · `title` · `statement` (EARS) · `rationale` · `binds` (code paths) ·
`verify` (tests / evidence) · `status` · `provenance` (decided / witness / evidence) ·
`references` (optional — external standard clauses / leading-practice sources a Daubert
challenge would want cited; each entry names the source, clause, and whether it is quoted
or pending re-confirmation).

**Status vocabulary:**
| status | meaning |
|---|---|
| `draft` | statement not yet ratified |
| `open` | statement ratified; code binding or automated verification incomplete |
| `bound` | code binding + automated CI verification both in place and passing |
| `retired` | superseded — never deleted; must point to its successor |

**CI trace-check** (`contract-trace`, planned): a governed REQ with no covering test, a
test citing a retired REQ, or a `capabilities.toml` claim with no REQ fails the build.
