# 0.8.29 memory acceptance (registered before implementation)

Scope: a separate PR stacked on the held auto-update host-gates branch. No
version bump, bootstrap pin regeneration, installation, release, or field-node
contact. Local tests are not native field proof.

1. The rootful engine service limits the entire split cgroup to 256 MiB,
   including conmon and charged tmpfs; swap is disabled. A systemd service
   drop-in may override the cap. OOM kills the service group and restarts it;
   three starts per five minutes stop an endless crash loop.
2. A root-owned host helper records only systemd-confirmed OOM deaths, once per
   invocation, atomically and durably outside engine-writable state. Ordinary
   stops/signals are not OOM events. Corruption is an error, not a reset to zero.
3. Read-only, bounded memory observations supply optional byte-valued engine
   RSS, container cgroup current/peak, console RSS, host total/available and
   volatile journal allocated bytes. Missing/malformed/stale data stays absent.
   A separately sampled host snapshot is boot-bound and expires in 90 seconds.
4. Heartbeat and local status share the same additive memory object. The console
   renders a nonzero event count as an attention item. Unknown old/new fields
   remain safe for globe and command consumers.
5. Behavioral RED precedes implementation; fixture tests, negative controls,
   installer embedded-body parity, unit generation, consumer tests, and a real
   ARM64 image build must pass. Run an isolated local service soak and allocation
   fault only within a capped cgroup. Field soak and native engine restart proof
   remain NOT_RUN until the existing operations gate is approved.

Limit choice: 256 MiB leaves substantial room above the measured 12–14 MB engine
RSS plus its 50 MiB capture tmpfs and conmon, without consuming the Pi's roughly
1.6 GB available RAM. This is a starting safety bound, not a measured peak.
