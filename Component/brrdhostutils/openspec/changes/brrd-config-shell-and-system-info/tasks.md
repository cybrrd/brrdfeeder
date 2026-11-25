# Tasks

## 1. Setup

- [x] 1.1 Add `publish = false` to all three crates and verify `cargo publish --dry-run -p brrd-core` refuses
- [x] 1.2 Add `ratatui`, `crossterm` to `brrd-config`, `sysinfo` to `brrd-core`, `insta` as a development dependency (`clap` is added in group 5), and verify `cargo build` succeeds
- [x] 1.3 Add a workspace lint configuration (clippy warnings as errors) and verify `cargo clippy --workspace -- -D warnings` passes

## 2. System information in brrd-core

- [x] 2.1 Define the snapshot types (value, unavailable, measuring) and verify with unit tests that each state formats as the specification requires
- [x] 2.2 Implement size and percentage formatting and verify with unit tests, including the `3.7 GiB` example
- [x] 2.3 Implement CPU, load and memory readings and verify with a test that runs on the build host and finds a core count above zero
- [x] 2.4 Implement the disk filter and verify with unit tests over fixed mount lists that kernel and memory-backed filesystems are excluded and `/` is kept
- [x] 2.5 Document the public operation in the crate's rustdoc and verify `cargo doc -p brrd-core` builds without warnings

## 3. Frame and navigation in brrd-config

- [x] 3.1 Implement the event thread (input and one-second tick) and verify the program starts, redraws on a tick, and exits on `q`
- [x] 3.2 Implement terminal setup and restore on every exit path and verify with a test that a provoked panic leaves the terminal restored
- [x] 3.3 Implement the frame (header, status line, body, hint line) and verify with a snapshot test at 80×24
- [x] 3.4 Implement the menu table and section lists and verify with snapshot tests for Status (one item) and Configuration (nothing available)
- [x] 3.5 Implement key-to-intent translation and list movement and verify each scenario under "Moving in a list" and "Opening and going back" with a test that uses a three-item test menu
- [x] 3.6 Implement number-key jumps and verify the "Jump from inside a page" and "Reserved number" scenarios
- [x] 3.7 Implement the too-small screen and verify with snapshot tests at 60×20 and after resize to 80×24
- [x] 3.8 Verify the "Monochrome terminal" scenario from the plain-text snapshots, with no test relying on color

## 4. System Info page

- [x] 4.1 Implement the page interface and the System Info page against a fixed snapshot source and verify with a snapshot test
- [x] 4.2 Connect the page to `brrd-core` on each tick and verify by running the program that values change without a key press
- [x] 4.3 Implement `unavailable` and `measuring` presentation and the status-line message and verify with snapshot tests for one failed value and for all values failed
- [x] 4.4 Add a "Building and running" section to README.md and verify the commands in it run as written

## 5. brrdctl status --system

- [x] 5.1 Add `clap` to `brrdctl`, define the `status` command with `--system`, and verify the `brrdctl-cli` scenarios for help, version, no arguments, unknown command and missing selector with tests that run the built program and check output and exit status
- [x] 5.2 Implement the one-shot report using `brrd-core` formatting and verify the "One-shot report" and "Output is plain" scenarios with a test that runs the program with output captured
- [x] 5.3 Verify with a test that `brrdctl status --system` and the System Info page print the same text for the same fixed snapshot
- [x] 5.4 Add `brrdctl status --system` to the README's "Building and running" section and verify the command runs as written

## 6. On a real BRRDfeeder

- [ ] 6.1 Record the mount table of a real feeder and verify the disk filter lists only its storage-backed filesystems, adjusting the filter and its tests if not
- [ ] 6.2 Build for ARM64, run `brrd-config` and `brrdctl status --system` on a real feeder over SSH in an 80×24 terminal, and verify every scenario in both specifications by hand, recording the result
- [ ] 6.3 Run `openspec validate brrd-config-shell-and-system-info --strict` and verify it passes

## 7. Corrections from the first run on a real feeder

- [x] 7.1 List each storage filesystem once, at the mount of its top directory, and verify with a unit test over the recorded feeder mount table that `/` and `/boot/firmware` are listed and the container storage mount is not
- [x] 7.2 Compute used space as total minus free and verify the "Reserve is not counted as used" scenario with a unit test, and "Agreement with df" with a test that runs `df` on the build host
- [x] 7.3 Remove a problem message from the status line when the next reading succeeds and verify with a snapshot test of a failed reading followed by a good one
- [x] 7.4 Add `.cargo/config.toml` naming the ARM64 linker and add the `aarch64-unknown-linux-gnu` target to `rust-toolchain.toml`, and verify `cargo build --release --target aarch64-unknown-linux-gnu` produces both programs
- [x] 7.5 Add the new scenarios to acceptance.md with the test that proves each, before writing the corrections
- [ ] 7.6 Repeat task group 6 on the real feeder after 7.1 to 7.3 and verify the disk list, the `df` comparison and a blank status line there

## Workflow follow-up

- Archive the change after review.
