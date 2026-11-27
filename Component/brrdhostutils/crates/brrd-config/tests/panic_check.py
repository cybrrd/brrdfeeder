# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
import os
import pty
import select
import subprocess
import sys
import termios
import time

master, slave = pty.openpty()
before = termios.tcgetattr(slave)
process = subprocess.Popen([sys.argv[1], '--exact', 'panic_probe', '--ignored', '--nocapture'],
                           stdin=slave, stdout=slave, stderr=slave,
                           env=dict(os.environ, TERM='xterm'))
try:
    output = b''
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.05)[0]:
            output += os.read(master, 65536)
        elif process.poll() is not None:
            break
    assert process.wait(timeout=1) != 0, output
    assert b'provoked terminal failure' in output, output
    assert b'\x1b[?1049h' in output, output
    assert b'\x1b[?25h' in output, output
    assert b'\x1b[?1049l' in output, output
    assert termios.tcgetattr(slave) == before
finally:
    if process.poll() is None:
        process.kill()
    process.wait()
    os.close(master)
    os.close(slave)
