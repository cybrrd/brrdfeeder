<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- SPDX-FileCopyrightText: 2026 Macawi LLC -->
# Helper 5 baseline freeze and planning citations

The implementation baseline is released-0.9.0 public main
`b4af3a9895f1cddafd82a91aeb115d4a4dfffdbc`. The planning checkout was
`25e203ab5bb3c500ad4df23ba94415b1ffbf8623`.

The four identity/build/upgrade/proof touchpoints below are byte-for-byte
identical between those revisions. The associated fixture and recovery files
are included for completeness. Each row was checked with `git show` byte
comparison and `git rev-parse REF:PATH`; `git diff` reports no changed lines.
Paths are relative to `Component/aviary/release-system/`.

| File | Planning lines | Frozen-baseline lines | Shared Git blob | Diff and meaning |
|---|---|---|---|---|
| `host_update.go` | 30–32 | 30–32 | `8c60db477eca4d32e5ab899c089fd411e082d3c5` | No changes; compiled helper identity/comment. |
| `build-host.sh` | 10, 16, 22–24 | 10, 16, 22–24 | `b433da9a767f83b71cf3e8d3680af5e1c5f1472e` | No changes; pinned compiler, linker identity and installer-pin verification. |
| `local_upgrade.go` | 72; 338–343; 412–414; 472–473 | 72; 338–343; 412–414; 472–473 | `e29be0fa84aeefdb8d0d76d7b6d7aa4d2a4d16c6` | No changes; success text, higher/equal-floor guards, exact candidate and floor write. |
| `tests/run-rework-proofs.py` | 34–35 | 34–35 | `d0a83fcb74f453a205dec40509b345bada09e450` | No changes; higher/equal-build mutation targets. |
| `local_upgrade_test.go` | 62; 198; 246 | 62; 198; 246 | `8cc44b32125efb46ef92052fbf0750e682d244ad` | No changes; primary fixture, success and crash recovery. |
| `local_layout_test.go` | 18; 48 | 18; 48 | `742ce248f8801965cb107a3691410e90ded2b915` | No changes; historical layout fixture and acceptance. |
| `launcher.py` | 107–121 | 107–121 | `3362b41e4b9f665d161e931771561d95fc0d3790` | No changes; journaled floor/absence restoration. |

The planning floor-write citation is **line 472**, with the following write/error
check starting at line 473; this corrects the earlier shorthand citation to
473 alone. These are historical baseline line numbers, not the helper-5 branch's
current line numbers. The helper-5 implementation intentionally changes the
identity, target checks, floor and corresponding tests after this freeze.

Reproduce the comparison from a checkout containing both commits:

```sh
git diff --exit-code 25e203ab5bb3c500ad4df23ba94415b1ffbf8623 \
  b4af3a9895f1cddafd82a91aeb115d4a4dfffdbc -- \
  Component/aviary/release-system/host_update.go \
  Component/aviary/release-system/build-host.sh \
  Component/aviary/release-system/local_upgrade.go \
  Component/aviary/release-system/tests/run-rework-proofs.py \
  Component/aviary/release-system/local_upgrade_test.go \
  Component/aviary/release-system/local_layout_test.go \
  Component/aviary/release-system/launcher.py
```

This source comparison does not establish a native host baseline or authorize
qualification. The [native kit](qualification/helper5/README.md) remains unbound
until its separate operator registration and approval.
