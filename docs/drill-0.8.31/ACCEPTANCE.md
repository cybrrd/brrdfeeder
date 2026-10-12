# Auto-update drill candidate acceptance — registered before implementation

Scope: two review-only candidate PRs. Bad `v0.8.31-drill.1` is a deliberate,
publicly identified console health failure, intended only for an approved dev
drill. Good `v0.8.31` remains healthy. No credential renewer is included.

1. Engine manifest and workspace lock agree exactly with each candidate version;
   existing release tag and SemVer validators accept both identities.
2. Bad console's production HTTP handler returns 503 on `/status`, even when
   supplied fresh healthy engine status. It remains a running read-only server;
   the root page and security checks remain functional. A behavioral assertion
   must fail on the unchanged baseline before introducing the failure.
3. Good console returns 200 with fresh healthy engine status and expected engine
   digest/revision. Its source and resulting console image differ from bad and N.
4. Execute the actual updater `consoleHealthy` against locally built candidate
   console processes: false for bad, true for good. Existing paired rollback,
   quarantine, health and floor tests remain passing. Local synthetic status
   does not constitute native P05–P10 evidence.
5. Failure must not modify host updater, pins, journals, image retention,
   credentials, radios or persisted application data. Explain the existing
   bounded health timeout and paired rollback; no claims of recovery from
   unrelated storage loss or external failures.
6. Preserve normal release provenance/signing gates. Explain tag/non-main
   eligibility, main dispatch dry run, protected environment, same-run receipts,
   and explicit GitHub prerelease selection. No actual tags, releases, merges,
   workflow runs, production/node contact, or signing in this work.
7. Register exact branch heads and history-derived builds. Good must have greater
   build than bad on both components; never assume later wall-clock tagging
   guarantees this. Give a merge strategy and mandatory post-merge count check.
8. Report PR links/heads, local checks and limitations, safe future release
   sequence and P05–P10 expectations. All native cases remain NOT_RUN.

Commits use `[skip ci]` to honor the no-workflow-runs restriction. Hosted CI and
actual ARM64/keyless signing require a later authorized release/review window.
