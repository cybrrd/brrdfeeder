<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Credential self-renewal implementation acceptance — registered before code

Authority: packet 2026-10-12; design #148 b3bbdf66 §9 and approved D-M/D-ALERT/D-ERR/D-GATE decisions. PRs only. No deployment, merge, tag, release, production/NATS/flock/Zitadel contact. Synthetic keys only. FULL-resolver migration, nine-key operations and n8n workflow are explicitly separate packets; provide interfaces/docs, not execution.

| Control / decision | Required executable proof |
| --- | --- |
| Local replacement key; MintForPublicKey | Flock signs supplied user subject; response contains JWT/public metadata only; engine assembles matching local seed |
| Revoked/non-latest/wrong-node/deregistered | Each refused after cryptographic proof and binding; corrupt/missing-ledger state fails closed; removal of each guard fails its named test |
| Grace / server clock | Exactly expiry+90×24h UTC inclusive; +1 second refused independently of client timestamp; future/skewed/invalid completion rejected |
| Durable challenge, 5min, replay | Challenge binds full tuple; dual signatures; one-use across restart/workers; store/cap/write failures return no JWT; mutations kill reuse/expiry/binding guards |
| Concurrency / issuance history | Concurrent renewals have one winner; registry-wide new-key uniqueness includes all nodes/history/revocations; same-new-key race has ≤1 issuance; owner issuance uses same invariant |
| Commit before disclosure | Mint/sign/append/fsync failure returns no JWT; challenge consumed durably before mint; crash recovery never repeats issuance |
| D-ERR | Generic pre-proof replies for unknown/revoked/other-node keys; actionable own-node reasons only after valid dual proof; no other node disclosed by key collision |
| D-M rate limits | Combined per-node and trusted-edge-IP budgets with Retry-After; bounded challenge table; synthetic load saturation and recovery |
| JWT SHOULD-1 | Wrong subject/issuer/account/signature, missing/expired/nonadvancing expiry, excessive lifetime/stale iat/nbf fail without changing active creds or reconnecting |
| Clock / daily jitter | Untrusted clock pauses and recovers; ≤30d triggers; daily stable jitter; bounded retry; ≤14d/blocked renewal appears in Red and Silver |
| Dedicated RW directory | Readonly secrets retained; narrow mount/UID/modes; same-directory0600 temp + file fsync + rename + dir fsync; bounded rollback copy, no automatic revoked-key fallback |
| Supervisor rotation | Explicit signal reconnects using new creds; queued frames preserved in order; control works during reconnect/backoff; failure stays pending |
| Owner fallback | Root required; existing device flow; identity unchanged; atomic credentials replacement/reconnect; no uninstall/state erase/seed output |
| D-ALERT interface | Narrow authenticated read-only active-latest roster; expiry/owner-action state, freshness stamp; corrupt/unavailable source errors, deregistered suppressed; no mint authority; no email/workflow implementation |
| D-GATE canary | Operator-only audited short-TTL issuance restricted to named canary; unavailable to ordinary enroll; isolated grace/refusal harness; documented later native canary→staging→general gates |
| Expired/offline recovery | Renewal transport independent of NATS; expired-within-grace succeeds, beyond-grace/revoked needs owner; OTA HTTPS path unchanged |
| Delivery/version | Public branch descends from healthy #46, never bad #45 failure; version 0.9.0 separate change; assert history-derived build > both drill candidates; merger sequencing documented |
| Out-of-scope §9 controls | FULL resolver push/failure/retry/migration/rollback, nine-key broker reconciliation, n8n dedupe/stale-source routing and native staged coverage remain NOT_RUN in explicitly separate packets; contract interfaces and runbook gates retained |

Register this file in both PRs before implementation. Record red→green and guard-mutation evidence; reject claims that synthetic tests establish native canary coverage or real credential renewal.
