# D26 intermediate fail-closed identity

**Step 4 is partially complete. This stops the lie; it does not make the
runtime digest known. Wrapper integration and real-image mutation proof remain
owed.** Install the updated command consumer first; no deployment is part of
this branch.

The engine no longer reads runtime `BRRDFEEDER_IMAGE_DIGEST`,
`BRRDFEEDER_GIT_SHA`, or `BRRDFEEDER_BUILD_SEQ` as running identity. There is no
verified handoff or binary-baked build metadata in this intermediate build.
Consequently it omits `image_digest`, `engine_version`, `build_seq`, and
`policy_ack` (no JSON null, empty string, fabricated zero, or last-known value).
`channel` remains a configuration assignment, defaulting to `stable`.

At startup the identity check emits one info line naming digest `unknown` and
source `unavailable`, plus one warning explaining the refusal. Each Silver
heartbeat includes:

```json
{"update_blocked_reason":"running_identity_unverified"}
```

This field is part of the heartbeat payload on
`cybrrd.system.node.heartbeat.<node>` and the reused local status snapshot,
not a substitute subject or a claimed policy acknowledgment. Existing transport,
heartbeat startup prerequisites, and cadence remain unchanged; this branch
does not prove broker delivery or device startup.

The engine's Blue gate returns `RejectedRunningIdentityUnverified` with no
policy passed to the draft writer, including for correctly signed rollback
requests. Boot reconciliation leaves pending drafts and persisted watermarks
unchanged when identity is unknown. A signed target is not evidence of the
running image. The existing standalone `verify-blue` signature/schema verifier
is unchanged: signature verification alone is not running-identity authorization.
Already-existing host-side drafts/updaters are outside this branch's control.

A future verified policy floor requires both actual image identity and its
bound build sequence, not just a nonzero sequence. Do not introduce an
environment fallback, equate image config IDs with manifest digests, or use
`policy_ack` as an alias for a version. PRV-001 §§5.2/5.4 distinguish recorded
custody and verifiable receipts from assertions; these tests do not establish
full provenance conformance.
