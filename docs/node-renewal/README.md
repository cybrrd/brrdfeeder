<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Automatic credential renewal (0.8.32 candidate)

Review-only implementation: no release, tag, deployment or production contact. The engine checks at startup and every 24 hours plus stable 0–6 hour node jitter; it renews at ≤30 days remaining. Failure backoff is exponential with jitter, capped at 24 hours, respecting bounded server `Retry-After`. Untrusted clock pauses renewal and checks again after 60 seconds.

The replacement user NKey is generated locally. Both keys sign a durable five-minute HTTPS challenge. No seed is sent through self-renewal. The server allows only the latest, unrevoked key for a non-deregistered node; grace ends at credential expiry plus exactly 90×24h UTC. HTTPS renewal does not require working NATS authentication.

## Trust and storage

The verified read-only installation credential supplies the independently provisioned account/signing-key trust anchor. The response cannot supply or change trust. A signing-key rollover requires an authenticated trust update first.

The verifier checks the NATS JWT signature, user type/version, expected node name, new seed-derived subject, trusted signing issuer and issuer account, response metadata, finite advancing expiry more than 30 days ahead, lifetime ≤90 days, `iat` within five minutes and `nbf`. Rejection leaves active bytes and the live connection unchanged.

Fresh installations use service-owned mode-0700 `/var/lib/brrdfeeder/credentials`, with mode-0600 `brrdfeeder.creds`. The installer adds its explicit narrow RW submount; `/etc/brrdfeeder/secrets` stays RO. Existing OTA-capable installations already mount state RW: the engine creates that private child and copies the authenticated bootstrap credential once, without changing the host config or making the secrets mount writable. Unsupported custom credential paths fail closed for automatic renewal and require operator migration. Storage/trust preparation failures retry locally.

Publication uses a same-directory exclusive temporary, file fsync, atomic rename and directory fsync. One bounded `previous.creds` is retained, never automatically reactivated. The supervisor receives an explicit rotation generation, rereads active credentials and reconnects while preserving its bounded queue/order, including reconnect/backoff. Red/Silver workers also drop their credential-bearing clients while retaining durable spools. New status schema: `cybrrd.credential-renewal.v1`; Red code: `credential_renewal_required`. Last-success is a process-local reconnect-confirmation timestamp, not a fleet coverage receipt.

Core NATS transport caveat: the existing async-nats `flush()` completes a client socket flush, **not an end-to-end broker acknowledgment**. Queue retention and handshake rotation tests do not claim transactional broker delivery; application delivery/JetStream acknowledgment and native broker proof remain distinct.

If the server commits issuance but its response is lost, rejected, or the node loses power before installing the new seed/JWT, the single-use protocol cannot replay that issuance. The old key may now be non-latest. Report the failure and use owner recovery; never silently roll back or relax the server's latest-key rule. Native power-loss/canary tests remain release prerequisites.

## Owner recovery without uninstalling

`sudo brrdfeeder renew` checks root and synchronized host time, then runs the capability-free helper already shipped inside the authenticated engine image. It presents the existing device-login approval link/code, calls owner enrollment with `renew_node_id`, verifies returned credentials, atomically installs them and sends SIGUSR1 for controlled reconnect. It does not rewrite node identity, erase captures, uninstall, or print seeds. A success message distinguishes installed/reconnect-requested from broker-confirmed.

Flock must implement the identity-pinning `renew_node_id` extension first. A different owner, renamed hostname, deregistered identity or privileged-remint binding can require operator intervention; identity mismatch is refused before minting. The automatic renewer ships OTA in the image, but the host command **cannot be added by an image-only update**. Existing nodes need the reviewed updated host command delivered separately; no host migration was performed here. The normal bootstrap pins were refreshed for this installer change.

## Tests and release gates

Registered controls: [ACCEPTANCE.md](ACCEPTANCE.md). Run the Rust 1.88 workspace tests offline and `python3 Component/brrdfeeder/install/tests/credential-renewal/test-contract.py`. `Component/aviary/engine/tests/renewal-mutations.py RECEIPT_DIRECTORY` requires named assertion failures for 17 verifier/storage/reconnect guard removals, not compilation errors. Synthetic loopback NATS wire peers verify actual JWT/key possession on reconnect and ordered delivery of frames retained while the broker connection was unavailable. Recorded guard/test pairs are in [mutation-summary.json](mutation-summary.json).

The companion server's TLS interoperability harness invokes the ignored `flock_lab_transport` unit test. It tests real Go challenge/mint responses against the Rust client for expired-within-grace success and beyond-grace/revoked refusal. It supplies only fresh loopback endpoints/certificates through a test-only path; production URLs cannot be overridden.

Not yet performed: native ARM ownership/Quadlet verification, forced short-TTL renewal on the designated dev canary, actual broker/Red/Silver confirmation, HTTPS OTA delivery to a node with expired credentials, power-loss injection on target storage, staging/general rollout and active-node coverage. These are release gates, not synthetic test passes. The server packet supplies operator-only short-TTL issuance and a read-only expiry roster; resolver migration, historical revocations and alarm workflow remain separate work.

History is based on healthy drill PR #46 (`09074b53712ba4d79038aaa9ec82b94c632a2e30`, build 1037), not the deliberately failing PR #45 branch (build 1035). Acceptance was committed first; implementation follows, with 0.8.32 on its own commit. Preserve this ancestry when integrating—do not squash/rebase it onto a lower build-count history. Release ordering remains bad drill → healthy drill → reviewed 0.8.32; this PR performs none of those releases. The 2026-12-15 coverage/reinstall checkpoint and 2027-01-04 deadline are unchanged.
