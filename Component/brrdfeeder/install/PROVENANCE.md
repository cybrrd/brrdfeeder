<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->

Account and host labels in this historical note are anonymized.
# Provenance — the customer install script, rescued 2026-09-21

**This file existed ONLY on `resonance:/srv/pack-stack/scripts/brrdfeeder-install.sh`.** It was not
in CYB1, not in any component, and not in any git remote. **The customer-facing installer — the one
artifact a paying subscriber actually runs — was a sole holder.** That is the rule March 18 cost us.

- Recovered: 1,114 lines, 49,679 bytes, sha256 `f7958fd01024c1a1f94f3ea23260c435da343c68848b437e2fe1e92dccd44d6f`
- Scanned for embedded secrets before commit: **none** — only path constants. The OAuth client id is
  a *public* identifier for a Native/PKCE app, not a secret.

**How it was nearly lost.** During the `brrdg3s1` decommission earlier the same day, the development team removed
`/home/operator/brrdfeeder-install.sh` from that node **without archiving it** — the archive captured
`/etc/brrdfeeder`, the quadlets and state, but not that path. A copy survived on two other nodes and
on resonance, so nothing was actually lost; **the process, not the luck, is what needs fixing.**

## What it actually does — and it is most of the PRIMARY path

| capability | present |
|---|---|
| installs packages (`apt-get install jq curl ca-certificates`) | ✅ |
| pulls the image from **`ghcr.io/macawi-ai/brrdfeeder-open:latest`** | ✅ |
| **Zitadel OAuth Device Flow** against `https://hospitality.cybrrd.com` | ✅ constants + flow |
| displays `user_code` + `verification_uri` for the user to authorise | ✅ |
| enrols with **flock** at `https://ingest.cybrrd.com/v1/enroll/oauth` | ✅ |
| stores `oauth_refresh.token` for 90-day credential renewal | ✅ |
| writes config, quadlet, secrets tree (`0700 root`) | ✅ |

**the development team's earlier readiness assessment was wrong** and is corrected here: it was made against
`Component/aviary/deploy/bootstrap/brrdfeeder-install.sh`, a *refresh/reconfigure* tool whose own
README says it "does not install packages or fetch an image." **That is a different script with a
confusingly similar name.** Two files named `brrdfeeder-install.sh` doing entirely different jobs is
itself a defect worth fixing.

## What still needs verifying — do NOT assume this is ready

Line 45 carries a comment reading *"ZITADEL OAUTH DEVICE FLOW — INTEGRATION POINT (next distinct
step)"*, which suggests the device flow may have been **planned rather than finished** when this was
written, even though the constants and a handler exist. **Whether the flow actually completes
end-to-end is unproven and is the first thing to establish** — by execution, on real hardware, not
by reading.
