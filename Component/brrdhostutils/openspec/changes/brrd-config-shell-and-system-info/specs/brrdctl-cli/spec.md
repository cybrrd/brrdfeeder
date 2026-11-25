## Purpose

`brrdctl` is the command-line front end of brrdtools, for power users, scripts and support. This capability covers how it is invoked and how it answers a command line it cannot act on, independent of what any one command reports.

## ADDED Requirements

### Requirement: Help
`brrdctl --help` SHALL list every available command with a one-line description, and `brrdctl <command> --help` SHALL list that command's options. Help SHALL list only what works in the running build.

#### Scenario: Top-level help
- **WHEN** `brrdctl --help` is run
- **THEN** it lists `status` with a one-line description and exits with status 0

#### Scenario: Command help
- **WHEN** `brrdctl status --help` is run
- **THEN** it lists `--system` with a one-line description and exits with status 0

### Requirement: Version
`brrdctl --version` SHALL print the program name and version and exit with status 0.

#### Scenario: Version
- **WHEN** `brrdctl --version` is run
- **THEN** it prints one line containing `brrdctl` and the version number

### Requirement: Mistakes are named
A command line that `brrdctl` cannot act on SHALL produce a message on standard error that names the problem and how to see the valid choices, and SHALL exit with status 2. Nothing SHALL be printed on standard output.

#### Scenario: Unknown command
- **WHEN** `brrdctl frobnicate` is run
- **THEN** standard error names `frobnicate` as unrecognized and points to `brrdctl --help`, and the exit status is 2

#### Scenario: Status without a selector
- **WHEN** `brrdctl status` is run with no option
- **THEN** standard error states that a report must be chosen and names `--system`, and the exit status is 2

### Requirement: No command runs without being asked
`brrdctl` run with no arguments SHALL print the same text as `brrdctl --help` and SHALL change nothing.

#### Scenario: No arguments
- **WHEN** `brrdctl` is run with no arguments
- **THEN** the help text is printed
