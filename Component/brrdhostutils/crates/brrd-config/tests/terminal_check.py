"""Exercise terminal lifecycle against a real pseudo-terminal, without a shell."""
import fcntl
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time
import unittest

BINARY = sys.argv.pop(1)

class Screen:
    def __init__(self):
        self.cells = [[' '] * 80 for _ in range(24)]
        self.row = self.col = 0
        self.pending = ''

    def feed(self, data):
        self.pending += data.decode('utf-8', errors='replace')
        while self.pending:
            if self.pending.startswith('\x1b'):
                match = re.match(r'\x1b\[([0-9;?]*)([A-Za-z~])', self.pending)
                if not match:
                    return
                params, code = match.groups()
                if code in ('H', 'f'):
                    coords = [int(v or '1') for v in params.split(';')]
                    self.row = coords[0] - 1
                    self.col = (coords[1] if len(coords) > 1 else 1) - 1
                elif code == 'J' and params == '2':
                    self.cells = [[' '] * 80 for _ in range(24)]
                self.pending = self.pending[match.end():]
                continue
            char, self.pending = self.pending[0], self.pending[1:]
            if char == '\r':
                self.col = 0
            elif char == '\n':
                self.row += 1
            elif char.isprintable():
                if 0 <= self.row < 24 and 0 <= self.col < 80:
                    self.cells[self.row][self.col] = char
                self.col += 1

    def lines(self):
        return [''.join(row).rstrip() for row in self.cells]

class TerminalTests(unittest.TestCase):
    def launch(self, args=None, env=None):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
        before = termios.tcgetattr(slave)
        process = subprocess.Popen(args or [BINARY], stdin=slave, stdout=slave, stderr=slave,
                                   env=dict(os.environ, TERM='xterm', **(env or {})))
        self.addCleanup(os.close, master)
        self.addCleanup(os.close, slave)
        def stop():
            if process.poll() is None:
                process.kill()
            process.wait(timeout=3)
        self.addCleanup(stop)
        return master, slave, before, process

    def read_until(self, master, needle, timeout=3):
        output = b''
        deadline = time.monotonic() + timeout
        while needle not in output and time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                output += os.read(master, 65536)
        self.assertIn(needle, output)
        return output

    def test_pty_exit_keys(self):
        for key in [b'0', b'q', b'\x03']:
            for page in [False, True]:
                with self.subTest(key=key, page=page):
                    master, slave, before, process = self.launch()
                    self.read_until(master, b'\x1b[?1049h')
                    if page:
                        os.write(master, b'\r')
                        self.read_until(master, b'Esc/Left back')
                    os.write(master, key)
                    output = self.read_until(master, b'\x1b[?1049l')
                    self.assertIn(b'\x1b[?25h', output)
                    self.assertEqual(process.wait(timeout=3), 0)
                    self.assertEqual(termios.tcgetattr(slave), before)

    def test_tick_redraw(self):
        master, _, _, process = self.launch()
        self.read_until(master, b'\x1b[?25l')
        while select.select([master], [], [], 0.2)[0]:
            os.read(master, 65536)
        started = time.monotonic()
        self.read_until(master, b'\x1b[?25l', timeout=2)
        self.assertGreater(time.monotonic() - started, 0.5)
        self.assertIsNone(process.poll())
        os.write(master, b'q')
        self.assertEqual(process.wait(timeout=3), 0)

    def test_pty_live_page(self):
        self.assertNotEqual(os.geteuid(), 0)
        master, _, _, process = self.launch()
        screen = Screen()
        def wait_for(predicate, timeout=2):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    screen.feed(os.read(master, 65536))
                if predicate(screen.lines()):
                    return screen.lines()
            self.fail('screen did not update: ' + repr(screen.lines()))
        wait_for(lambda lines: lines[2] == 'Status')
        os.write(master, b'\r')
        lines = wait_for(lambda lines: any(line.startswith('Last reading: ') for line in lines))
        self.assertEqual(lines[2], 'Status > System Info')
        self.assertTrue(any(line.startswith('CPU cores: ') and 'unavailable' not in line for line in lines))
        self.assertTrue(any(line.startswith('  /: ') and 'unavailable' not in line for line in lines))
        old_memory = next(line for line in lines if line.startswith('Memory: '))
        old_time = next(line for line in lines if line.startswith('Last reading: '))
        allocation = bytearray(512 * 1024 * 1024)
        allocation[::4096] = b'x' * (len(allocation) // 4096)
        lines = wait_for(lambda lines: any(line.startswith('Memory: ') and line != old_memory for line in lines)
                         and any(line.startswith('Last reading: ') and line != old_time for line in lines)
                         and any(re.fullmatch(r'CPU usage: [0-9]+%', line) for line in lines))
        self.assertTrue(any(re.fullmatch(r'CPU usage: [0-9]+%', line) for line in lines))
        del allocation
        os.write(master, b'q')
        self.assertEqual(process.wait(timeout=3), 0)

if __name__ == '__main__':
    unittest.main()
