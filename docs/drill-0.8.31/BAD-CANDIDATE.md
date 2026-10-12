# Deliberately unhealthy v0.8.31-drill.1 — do not merge

This branch is exclusively a review/tag candidate for a separately authorized
dev-ring rollback drill. Never merge it or promote its images. Publish it only
as a GitHub prerelease, never as latest. The healthy candidate is a separate PR.

The actual console process returns HTTP 503 for `/status`, after the existing
host/method/origin checks. `/` remains usable and the server stays running.
There is no flag to accidentally enable the failure in another build: only this
branch's main enables it. Engine version and workspace lock truthfully identify
`0.8.31-drill.1`; no false OCI identity or signature bypass is used.

The unchanged host updater requires HTTP 200, a fresh healthy rendered engine,
and matching digest/revision (`release-system/package.go:93–120`). Repeated 503
responses cannot pass. After its 180-second console health window it enters
`failedUpdate` (lines 383–384), restores both prior pins and retained actual
images, and checks their health (lines 465–565). Engine and console windows are
separate, each up to 180 seconds; this is not a 180-second end-to-end SLA.
Two failed applications quarantine the exact digest pair regardless of later
sequence/salt changes. Nothing here alters host helper, filesystem state,
retention anchors, credentials, radios or data. Recovery still depends on the
registered healthy baseline, retained images and functioning host/storage.

Acceptance was committed before implementation. The local production-process
negative control on the unchanged console returned `200, want 503`, as expected.
`bash docs/drill-0.8.31/prove-console.sh false` runs the built console against
the actual updater health function using synthetic status and simulated time.
Run with the pinned Go 1.27.0 compiler; no node contact or tag is required.
The companion healthy branch uses the identical proof with argument `true`.

The workflow's draft creator does not infer prerelease from the tag. The release
approver must explicitly set prerelease=true and latest=false before publishing.
Full future sequence and native observations are supplied in the private handoff.
Hosted CI is intentionally skipped under the current no-workflow-runs limit;
real release CI/signing, native rollback and received telemetry remain unproved.
