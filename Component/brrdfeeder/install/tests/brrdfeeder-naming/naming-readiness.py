# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Synthetic readiness for the existing disposable naming OS fixture only."""
import datetime
import json
import os
from pathlib import Path
import yaml

assert os.environ.get('BRRD_NAMING_CONTAINER') == '1'
assert os.geteuid() == 0 and Path('/run/.containerenv').exists()
config = yaml.safe_load(Path('/etc/brrdfeeder/config.yaml').read_text())
status = dict(schema_version=1, status_interval_secs=30,
              written_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
              heartbeat=dict(node_id=config['node']['id'], gps=dict(state='healthy'), radio_status='up'),
              links=dict(nats_state='connected'), inventory=dict(capture=[dict(monitor_mode=True)]))
Path('/var/lib/brrdfeeder-status/status.json').write_text(json.dumps(status))
print('ActiveState=active\nSubState=running\nInvocationID='+'a'*32)
