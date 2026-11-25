## Purpose

System information reports the state of the computer a BRRDfeeder runs on: how busy its processor is, how much memory is in use, and how full its storage is. It is read-only and is the same information whichever tool presents it.

## ADDED Requirements

### Requirement: CPU report
The snapshot SHALL report the number of processor cores, the share of processor time in use across all cores as a percentage, and the 1, 5 and 15 minute load averages.

#### Scenario: CPU shown on the page
- **WHEN** the System Info page is open
- **THEN** it shows the core count, the percentage in use and the three load averages

#### Scenario: First reading
- **WHEN** the page has just been opened and a usage percentage cannot yet be calculated
- **THEN** the percentage is shown as `measuring` and no number is shown in its place

### Requirement: Memory report
The snapshot SHALL report memory in use and total memory, and swap in use and total swap. A host with no swap SHALL be reported as having none.

#### Scenario: Memory shown on the page
- **WHEN** the System Info page is open
- **THEN** it shows memory used, total and the percentage used, and the same for swap

#### Scenario: No swap
- **WHEN** the host has no swap configured
- **THEN** swap is shown as `none`

### Requirement: Disk report
The snapshot SHALL report each filesystem that is backed by a storage device once, with its mount point, space used, total space and percentage used. A filesystem that is mounted at more than one place SHALL be listed only at the place where its top directory is mounted. Filesystems that exist only in memory or are provided by the kernel SHALL NOT be listed.

#### Scenario: Root filesystem
- **WHEN** the System Info page is open
- **THEN** the filesystem mounted at `/` is listed with used, total and percentage

#### Scenario: Kernel filesystems excluded
- **WHEN** the host has `/proc`, `/sys` and a memory-backed `/run` mounted
- **THEN** none of them is listed

#### Scenario: Filesystem mounted a second time
- **WHEN** the device mounted at `/` is also mounted, from one of its subdirectories, at `/var/lib/containers/storage/overlay`
- **THEN** `/` is listed and `/var/lib/containers/storage/overlay` is not

#### Scenario: Feeder with its containers running
- **WHEN** the report is produced by an ordinary user on a BRRDfeeder whose containers are running
- **THEN** no disk entry is shown as `unavailable`

### Requirement: Used space is the space files occupy
Space used SHALL be the filesystem's total space minus its free space, so that space the filesystem holds in reserve for the administrator is not counted as used. The percentage SHALL be used space divided by total space. The figures SHALL agree with those the host's `df` command reports for the same filesystem.

#### Scenario: Reserve is not counted as used
- **WHEN** a filesystem has 234.0 GiB in total, 229.5 GiB free, and 220.0 GiB available to an ordinary user
- **THEN** it is shown as `4.5 GiB / 234.0 GiB (2%)`

#### Scenario: Agreement with df
- **WHEN** the report and `df` are run on the same host within a few seconds
- **THEN** used space for `/` differs between them by no more than 0.1 GiB

### Requirement: Refreshes while open
While the System Info page is open, its values SHALL be refreshed at least once every two seconds without any key being pressed. The page SHALL show the time of the last successful reading.

#### Scenario: Value changes without input
- **WHEN** memory in use changes while the page is open and no key is pressed
- **THEN** the page shows the new value within two seconds

#### Scenario: Time of reading
- **WHEN** the page is open
- **THEN** it shows the time at which the displayed values were read

### Requirement: Unreadable values are stated, never guessed
A value that cannot be read SHALL be shown as `unavailable`. It SHALL NOT be shown as zero, and a value from an earlier reading SHALL NOT be shown as if it were current. The values that can be read SHALL still be shown.

#### Scenario: One value fails
- **WHEN** the load averages cannot be read and memory can
- **THEN** the load averages are shown as `unavailable` and memory is shown normally

#### Scenario: Everything fails
- **WHEN** no system information can be read
- **THEN** every value is shown as `unavailable` and the program keeps running

### Requirement: Consistent units
Sizes SHALL be shown in binary units (MiB, GiB) with one decimal place, and percentages as whole numbers.

#### Scenario: Memory size
- **WHEN** total memory is 3,884,000 KiB
- **THEN** it is shown as `3.7 GiB`

### Requirement: Same report from the command line
`brrdctl status --system` SHALL print the CPU, memory and disk report once, as plain text, and exit. It SHALL use the same values, names and units as the System Info page. It SHALL print a CPU usage percentage, waiting up to two seconds to measure one.

#### Scenario: One-shot report
- **WHEN** `brrdctl status --system` is run
- **THEN** it prints the core count, CPU percentage, load averages, memory, swap and each listed filesystem, then exits with status 0

#### Scenario: Same units as the page
- **WHEN** total memory is 3,884,000 KiB
- **THEN** `brrdctl status --system` prints it as `3.7 GiB`

#### Scenario: Unreadable value on the command line
- **WHEN** the load averages cannot be read
- **THEN** they are printed as `unavailable`, the other values are printed normally, and the exit status is 0

#### Scenario: Output is plain
- **WHEN** the output is sent to a file or a pipe
- **THEN** it contains no color codes or cursor movement

### Requirement: Read-only and unprivileged
Reading system information SHALL NOT change anything on the host and SHALL NOT require administrator rights. It SHALL work on a host where no feeder software is installed.

#### Scenario: Ordinary user
- **WHEN** `brrd-config` is run by a user without administrator rights and the System Info page is opened
- **THEN** CPU, memory and disk values are shown

#### Scenario: Ordinary user on the command line
- **WHEN** `brrdctl status --system` is run by a user without administrator rights
- **THEN** CPU, memory and disk values are printed

#### Scenario: No feeder installed
- **WHEN** `brrd-config` is run on a Linux host with no feeder software
- **THEN** the System Info page shows CPU, memory and disk values
