#!/usr/bin/env python3
"""Rebase/customer wording contracts; no install or host mutation."""
from pathlib import Path
import re
import unittest

ROOT=Path(__file__).resolve().parents[5]
INSTALL=ROOT/'Component/brrdfeeder/install'

class Integration(unittest.TestCase):
    def test_ending_names_and_explains_self_update(self):
        text=(INSTALL/'install-log.py').read_text()
        ending=text.split('def final_install_screen(',1)[1].split('\nclass Progress:',1)[0]
        self.assertIn('Self-Update',ending)
        self.assertRegex(ending,r'signed updates')
        self.assertRegex(ending,r'previous version|rolls back')
        self.assertNotRegex(ending,r'\b(?:D44|D40|Pack|Drop|effector|NotValidYet)\b')

    def test_new_updater_messages_are_customer_words(self):
        text=(INSTALL/'brrdfeeder-install.sh').read_text()
        section=text.split('# Step 5.5',1)[1].split('# Step 6',1)[0]
        messages='\n'.join(line for line in section.splitlines() if re.search(r'\b(?:gate|say|ok|warn|fatal) ',line))
        self.assertNotRegex(messages,r'\b(?:D44|D40|effector|self-care)\b')

if __name__=='__main__': unittest.main(verbosity=2)
