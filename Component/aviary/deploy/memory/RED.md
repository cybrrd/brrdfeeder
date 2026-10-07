# Behavioral RED — 2026-10-07 UTC

Base: a02976123ed42fe2918d9c3fb55f5b7223f588a0 (0.8.29 host gates).

`go test -run TestMemoryEventIsVisible -count=1 ./...` in Component/brrdhouse
compiled and exited 1: `memory event was silently discarded`. The actual HTTP
page rendered “Engine running”, attention count 0, and omitted both the supplied
two-event count and memory-limit warning. This is a behavioral assertion failure,
not a missing-symbol or compiler failure. No production code had been edited.

The later synthetic OOM test must only allocate inside an enforced memory cgroup;
an uncapped baseline OOM test is intentionally forbidden.
