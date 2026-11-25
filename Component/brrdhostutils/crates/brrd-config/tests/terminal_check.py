"""Exercise terminal lifecycle against a real pseudo-terminal, without a shell."""
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time
import unittest

BINARY = sys.argv.pop(1)

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

if __name__ == '__main__':
    unittest.main()
