## Purpose

The shell is the part of `brrd-config` that is the same on every screen: the frame, the sections, the keys that move between and within them, and how the program starts, exits and copes with the terminal it is given.

## ADDED Requirements

### Requirement: Fixed frame
Every screen SHALL be drawn inside the same frame, from top to bottom: a header line, a status line, a body, and a key hint line. Only the body SHALL change when the operator moves between sections, lists and pages.

#### Scenario: Frame at start
- **WHEN** `brrd-config` starts in an 80×24 terminal
- **THEN** the header is on the first line, the status line on the second, the key hint line on the last, and the body fills the space between

#### Scenario: Frame while a page is open
- **WHEN** the operator opens Status > System Info
- **THEN** the header, status line and key hint line remain in the same positions

### Requirement: Header shows the program and the sections
The header SHALL show the program name `brrd-config` and the five sections with their number keys: `1` Status, `2` Configuration, `3` Events, `4` Advanced Tools, `5` About. The section the operator is in SHALL be marked.

#### Scenario: Current section is marked
- **WHEN** the operator is anywhere inside the Status section
- **THEN** Status is the only section marked as current in the header

#### Scenario: Header fits 80 columns
- **WHEN** the terminal is 80 columns wide
- **THEN** the program name and all five sections are fully visible in the header

### Requirement: Number keys jump to a section
Pressing `1` to `5` SHALL take the operator to the top of that section from any screen, including from inside a page. Keys `6` to `9` SHALL do nothing.

#### Scenario: Jump from inside a page
- **WHEN** the operator is on Status > System Info and presses `5`
- **THEN** the body shows the About section and About is marked in the header

#### Scenario: Reserved number
- **WHEN** the operator presses `7`
- **THEN** the screen does not change

### Requirement: Starts in Status
When started, `brrd-config` SHALL show the Status section.

#### Scenario: First screen
- **WHEN** `brrd-config` starts
- **THEN** the body shows the Status section list and Status is marked in the header

### Requirement: A section lists only what works
The body of a section SHALL list only the items available in the running build. A section with no available items SHALL say that nothing is available in this version, in words.

#### Scenario: Status section in this build
- **WHEN** the operator presses `1`
- **THEN** the list contains System Info and no item that cannot be opened

#### Scenario: Section with nothing built
- **WHEN** the operator presses `2`
- **THEN** the body states that nothing is available in this version, and no list is shown

### Requirement: Moving in a list
The up and down arrow keys SHALL move the highlight one item at a time. The highlight SHALL stop at the first and last items. Exactly one item SHALL be highlighted whenever a list is shown.

#### Scenario: Move down
- **WHEN** a list of three items is shown with the first highlighted and the operator presses the down arrow
- **THEN** the second item is highlighted

#### Scenario: Stop at the end
- **WHEN** the last item is highlighted and the operator presses the down arrow
- **THEN** the last item remains highlighted

### Requirement: Opening and going back
Enter or the right arrow SHALL open the highlighted item. Esc or the left arrow SHALL return to the list the item was opened from, with the same item highlighted. At the top of a section, Esc and the left arrow SHALL do nothing.

#### Scenario: Open with Enter
- **WHEN** System Info is highlighted and the operator presses Enter
- **THEN** the body shows the System Info page

#### Scenario: Open with right arrow
- **WHEN** System Info is highlighted and the operator presses the right arrow
- **THEN** the body shows the System Info page

#### Scenario: Back
- **WHEN** the System Info page is open and the operator presses Esc
- **THEN** the body shows the Status list with System Info highlighted

#### Scenario: Back at the top of a section
- **WHEN** the Status list is shown and the operator presses the left arrow
- **THEN** the screen does not change

### Requirement: The body says where the operator is
The first line of the body SHALL name the current location as a path from the section, for example `Status` or `Status > System Info`.

#### Scenario: Path on a page
- **WHEN** the System Info page is open
- **THEN** the first line of the body reads `Status > System Info`

### Requirement: Key hints match the screen
The key hint line SHALL list the keys that have an effect on the current screen, and no key that has none.

#### Scenario: Hints on a list
- **WHEN** a section list is shown
- **THEN** the hint line names the keys for moving, opening, jumping to a section and exiting

#### Scenario: Hints on a page
- **WHEN** the System Info page is open
- **THEN** the hint line names the key for going back and does not name the key for opening

### Requirement: Exit
Pressing `0`, `q` or Ctrl-C SHALL end the program from any screen. On exit, for any reason including an internal failure, the terminal SHALL be returned to the state it was in before the program started.

#### Scenario: Exit with zero
- **WHEN** the operator presses `0` on any screen
- **THEN** the program ends with exit status 0 and the shell prompt is usable

#### Scenario: Exit after a failure
- **WHEN** the program ends because of an internal failure
- **THEN** the terminal echoes typed characters and shows the cursor, as it did before the program started

### Requirement: Terminal too small
In a terminal smaller than 80 columns by 24 rows, `brrd-config` SHALL show a message stating the size it needs and the current size, instead of a cut-off frame. The exit keys SHALL still work, and the frame SHALL return when the terminal is made large enough.

#### Scenario: Started too small
- **WHEN** `brrd-config` starts in a 60×20 terminal
- **THEN** the screen states that 80×24 is needed and that the terminal is 60×20

#### Scenario: Enlarged
- **WHEN** the terminal is resized from 60×20 to 80×24
- **THEN** the full frame is shown

### Requirement: Usable without color
The current section, the highlighted item and any warning SHALL each be distinguishable by a character or by position, not by color alone.

#### Scenario: Monochrome terminal
- **WHEN** the screen is captured as plain text with no color information
- **THEN** the current section and the highlighted item can each be identified from the text

### Requirement: Status line carries messages
The status line SHALL show the most recent message the program has for the operator, and SHALL be blank when there is none. A message that reports a problem SHALL be removed when the problem is no longer present.

#### Scenario: Nothing to report
- **WHEN** `brrd-config` has started and no problem has occurred
- **THEN** the status line is blank

#### Scenario: A reading fails
- **WHEN** the System Info page is open and the system information cannot be read
- **THEN** the status line states that system information could not be read

#### Scenario: The problem clears
- **WHEN** a reading failed and the next reading succeeds
- **THEN** the status line is blank

#### Scenario: Healthy feeder
- **WHEN** the System Info page is opened by an ordinary user on a BRRDfeeder whose containers are running
- **THEN** the status line is blank
