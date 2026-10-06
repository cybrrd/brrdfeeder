#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Offline structural guards; real teardown runs only in disposable OS fixtures."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[5]
INSTALLER=ROOT/'Component/brrdfeeder/install/brrdfeeder-install.sh'
SOURCE=INSTALLER.read_text()
HELPER=(INSTALLER.parent/'uninstall.sh').read_text()

class Contract(unittest.TestCase):
    def test_dry_run_has_no_completed_action_claims(self):
        # Execute the actual dry branches without invoking the privileged helper
        # on the test host. act() is real; dry=1 keeps its commands unexecuted.
        def function(name):
            lines=HELPER.splitlines()
            start=next(i for i,line in enumerate(lines) if line.startswith(name+'() {'))
            if lines[start].endswith('}'): return lines[start]
            end=next(i for i in range(start+1,len(lines)) if lines[i]=='}')
            return '\n'.join(lines[start:end+1])
        policy=re.search(r"^log 'Local uninstall.*?(?=^# The public installer)",HELPER,re.M|re.S).group()
        adoption=HELPER.split('      log "legacy account recognised:',1)[1].split('\n',1)[0]
        adoption='log "legacy account recognised:'+adoption
        manager=HELPER.split('act systemctl daemon-reload\n',1)[1].split('\n\n# Only package containers',1)[0]
        accounts=HELPER.split('for user in brrdhouse brrdfeeder; do\n',1)[1].split('\nremove_tree /etc/brrdfeeder',1)[0]
        script='set -euo pipefail\ndry=1\ndeclare -A visited_files=()\n'
        script+='\n'.join(function(n) for n in ['log','act','absent','exists','remove_file','remove_tree','stop_unit','remove_images'])
        script+='\nsafe_tree() { :; }\ngetent() { return 2; }\n'
        script+='declare -A uid=([brrdfeeder]=600001 [brrdhouse]=600002) gid=([brrdfeeder]=600001 [brrdhouse]=600002)\n'
        script+='user=brrdfeeder; id=600001; group=600001\n'+adoption+'\n'+policy+'\n'+manager+'\n'
        script+='\nstop_unit system brrdfeeder-engine.service\nremove_images system fixture/repo fixture-container\n'
        script+='remove_file "$1"\nremove_tree "$2"\nfor user in brrdhouse brrdfeeder; do\n'+accounts+'\n'
        with tempfile.TemporaryDirectory() as directory:
            file=Path(directory)/'keep'; file.write_text('unchanged')
            result=subprocess.run(['bash','-c',script,'dry-wording',str(file),directory],text=True,capture_output=True)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(file.read_text(),'unchanged')
        forbidden=r'path removal:|account removal:|\bREMOVED (?:image|tree|empty directory|path|account)|entries removed|Quadlet source is removed|Local uninstall complete|^\[brrdfeeder-uninstall\] EXPLICIT LEGACY ADOPTION'
        bad=[line for line in result.stdout.splitlines() if not re.search(r'\bwould\b',line,re.I) and re.search(forbidden,line,re.I)]
        self.assertFalse(bad,'dry-run claimed completed actions:\n'+'\n'.join(bad))
        self.assertIn('WOULD:',result.stdout)

    def test_self_contained_exact_uninstaller(self):
        body=SOURCE.split("<<'UNINSTALL_EOF'\n",1)[1].split('\nUNINSTALL_EOF',1)[0]+'\n'
        self.assertEqual(body,HELPER)

    def test_quadlet_autoremove_race_accepts_only_verified_absence(self):
        body=HELPER.split('remove_images() {',1)[1].split('\n}\n',1)[0]
        script='''set -euo pipefail
dry=0
declare -A uid=()
log() { echo "$*"; }
die() { echo "$*"; exit 1; }
absent() { echo "nothing to do: $* absent"; }
podman() { :; }
removed=0
pod() {
  shift
  case "$1 $2" in
    'container exists') (( ! removed ));;
    'inspect fixture') echo ghcr.io/cybrrd/brrdfeeder@sha256:fixture;;
    'stop fixture') removed=$1_REMOVED;;
    'rm fixture') return 1;;
    'images --no-trunc') :;;
    *) echo "unexpected fixture call $*" >&2; exit 9;;
  esac
}
remove_images() {'''+body+'\n}\nremove_images system ghcr.io/cybrrd/brrdfeeder fixture\n'
        for removed,expected in [(1,0),(0,1)]:
            result=subprocess.run(['bash','-c',script.replace('$1_REMOVED',str(removed))],text=True,capture_output=True)
            self.assertEqual(result.returncode,expected,result.stdout+result.stderr)

    def test_early_offline_dispatch(self):
        self.assertLess(SOURCE.index("<<'INSTALL_LOG_PY'"),SOURCE.index("<<'UNINSTALL_EOF'"))
        self.assertLess(SOURCE.index("<<'UNINSTALL_EOF'"),SOURCE.index('gate pre-flight "Pre-flight"'))
        self.assertNotIn('tee -a',HELPER)  # no secondary unredacted audit sink
        # A printed recovery hint is not network contact. Executable helper lines
        # remain network-free; the confirmation includes a bounded read timeout.
        self.assertNotRegex(HELPER,r'(?m)^\s*(?:exec |act |run_step \S+ )?curl\s')
        self.assertNotIn('podman pull',HELPER)
        self.assertNotIn('rm -rf', '\n'.join(l for l in HELPER.splitlines() if not l.lstrip().startswith('#')))
        self.assertNotIn('userdel -r',HELPER)
        self.assertNotIn('apt-get remove',HELPER)

    def test_install_steps_unchanged_outside_registered_gps_scope(self):
        # Frozen normalized pre-change flow; no private Git history is needed.
        old=(Path(__file__).parent/'fixtures/install-flow.txt').read_text().rstrip('\n')
        # Explicit GPS/P0 exceptions; enrollment internals, image pins/pulls,
        # account creation, host config, updater and other confinement stay equal.
        def normalize(text):
            # Public-directory traversal is independently exercised under
            # uutils parent semantics in directory-modes/test-contract.py.
            text=text.replace('public_directories /usr/local /usr/local/libexec',
                              'run install -d -m 0755 -o root -g root /usr/local/libexec')
            text=text.replace('public_directories /etc/containers "$QUADLET_DIR"',
                              '[[ -d "$QUADLET_DIR" ]] || run install -d -m 0755 "$QUADLET_DIR"')
            text=text.replace('public_directories /etc/containers /etc/containers/systemd /etc/containers/systemd/users \\\n    "/etc/containers/systemd/users/$CONSOLE_UID"',
                              'run install -d -m 0755 -o root -g root "/etc/containers/systemd/users/$CONSOLE_UID"')
            text=text.split('gate pre-flight "Pre-flight"\n',1)[1]
            # 2026-09-28 item 3: final verification and power/clock blocks have
            # behavioral coverage in installer-vcgencmd; preserve all other
            # enrollment, privileges, image, account and updater code checks.
            text=text.split('gate verification',1)[0]
            for start,end in [('# --- Power-supply sanity', '# --- Legacy migration'),
                              ('gate clock ', '# Step 3 — udev rules')]:
                a=text.index(start); b=text.index(end,a)
                text=text[:a]+text[b:]
            text=text.replace('INSTALL_ENGINE_RESTART_AT=$(date +%s)\n','')
            text=text.replace('updates use the signed-update service', 'updates belong to the Blue/updater path')
            text=text.replace('Signed-update watcher is ready (brrdfeeder-updater.path active/waiting)',
                              'host-updater ARMED (brrdfeeder-updater.path active/waiting — self-care enabled)')
            text=text.replace('Signed-update watcher is inactive; automatic updates are unavailable.',
                              'host-updater NOT armed (brrdfeeder-updater.path inactive — node cannot self-update via Blue)')
            text=text.replace('Checking the update signature against the trusted signing key…',
                              're-verifying signed Blue against the pinned S5 key…')
            text=text.replace('Description=cyBRRD BRRDfeeder engine (containerized) — Wave 8.0',
                              'Description=cyBRRD BRRDfeeder engine (containerized)')
            text=text.replace(' (Drop 2 effector)','')
            text=text.replace('verify + pull + pin + restart on a signed Blue','verify and install a signed update')
            text=text.replace('host-updater self-care effector','signed-update helper')
            text=text.replace('stop any existing user-mode service (Wave 7.x migration)',
                              'stop any existing legacy user-mode service')
            text=text.replace('''  if [[ -f "$CHRONY_DROPIN" || -f "$LEGACY_CHRONY_DROPIN" ]]; then
    ok "Clock correction settings present"
  else
    warn "Clock correction settings absent; re-run the installer."
  fi''','''  [[ -f "$CHRONY_DROPIN" ]]     && ok "chrony makestep hardening present"     || warn "chrony hardening ABSENT"''')
            text=text.replace('https://get.cybrrd.com | bash','https://get.cybrrd.com | sudo bash')
            # BLE-rfkill packet: independently exercised summary and durable hints.
            # One-liner rework keeps the same health summary visible in quiet mode.
            text=text.replace('  log_event NOTICE "RID.BLE Waiting:', '  say "RID.BLE Waiting:')
            summary='if [[ $GPS_WAITING -eq 1 ]]; then\n  say "RID.BLE Waiting:'
            if summary in text:
                a=text.index(summary); b=text.index('\nfi\n',a)+4
                text=text[:a]+text[b:]
            text=text.replace('Must run as root. Use: curl -fsSL https://get.cybrrd.com | sudo bash',
                              'Must run as root (sudo bash $0). Condo Tenancy: this is THE tenant-wall crossing.')
            text=text.replace('To re-run (idempotent):     curl -fsSL https://get.cybrrd.com | sudo bash','To re-run (idempotent):     sudo bash $0')
            text=text.replace('To verify without changes:  sudo brrdfeeder --verify','To verify without changes:  sudo bash $0 --verify')
            # Simple-uninstall adds only its early self-contained recovery copy;
            # exercised separately by simple-uninstall's disposable OS acceptance.
            if 'install_local_command() {' in text:
                a=text.index('install_local_command() {')
                b=text.index('# Never resolve a registry tag here',a)
                text=text[:a]+text[b:]
            # 0.8.23 changes publication only; durability and interrupted-state
            # behavior run against real files/accounts in test-container.py.
            for before,after in [
                ('atomic_install 0644 root root "$CONFIG_PATH" "$CONFIG_PATH" sed "s/', 'run sed -i "s/'),
                ('run atomic_install 0644 root root "$CONFIG_PATH" "$LEGACY_CONFIG"','run install -m 0644 -o root -g root "$LEGACY_CONFIG" "$CONFIG_PATH"'),
                ('run atomic_install 0600 root root "$CREDS_PATH" "$LEGACY_CREDS"','run install -m 0600 -o root -g root "$LEGACY_CREDS" "$CREDS_PATH"'),
                ('atomic_install 0755 root root "$STATUS_PROVISIONER"','cat > "$STATUS_PROVISIONER"'),
                ('atomic_install 0755 root root "$IDENTITY_INSTALL"','cat > "$IDENTITY_INSTALL"'),
                ('atomic_install 0644 root root "$CONSOLE_QUADLET_FILE"','cat > "$CONSOLE_QUADLET_FILE"'),
                ('run atomic_install 0644 root root "$backup" "$QUADLET_FILE"','run cp "$QUADLET_FILE" "$backup"'),
                ('''printf '%s\\n' "$NEW_UDEV_CONTENT" | atomic_install 0644 root root "$UDEV_RULES_FILE"''','echo "$NEW_UDEV_CONTENT" > "$UDEV_RULES_FILE"'),
                ('''printf '%s\\n' "$NEW_QUADLET" | atomic_install 0644 root root "$QUADLET_FILE"''','echo "$NEW_QUADLET" > "$QUADLET_FILE"'),
                ('''printf '%s\\n' "$creds_body" | atomic_install 0640 root "$TARGET_GID" "$CREDS_PATH"''','''printf '%s\\n' "$creds_body" > "$CREDS_PATH"
  run chmod 0640 "$CREDS_PATH"; run chown root:"$TARGET_GID" "$CREDS_PATH"'''),
                ('''printf '%s\\n' "$refresh_token" | atomic_install 0600 root root "$REFRESH_TOKEN_PATH"''','''printf '%s\\n' "$refresh_token" > "$REFRESH_TOKEN_PATH"
    run chmod 0600 "$REFRESH_TOKEN_PATH"; run chown root:root "$REFRESH_TOKEN_PATH"''')]:
                text=text.replace(before,after)
            text=text.replace('run sed -i "s/^\\(\\s*id:\\s*\\).*/\\1\\"${assigned_id}\\"/"\n',
                              'run sed -i "s/^\\(\\s*id:\\s*\\).*/\\1\\"${assigned_id}\\"/" "$CONFIG_PATH"\n')
            text=text.replace('''    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(directory)
    finally: os.close(directory)
''','')
            text=text.replace(' || $BOOT_PREPARE -eq 1 ]]',' ]]')
            text=text.replace("  if [[ $BOOT_PREPARE -eq 1 && \"$REQUESTED_IMAGE\" != \"$INSTALLED_IMAGE\" ]]; then say 'Repairing host setup; retaining the installed engine digest for signed Self-Update.'; fi\n",'')
            text=text.replace(' To install this release: sudo brrdfeeder uninstall, then run the one-liner again.','')
            text=text.replace('updates belong to the signed release poller.', 'updates belong to the Blue/updater path.')
            text=text.replace('# Step 5.5 — Install independent signed-release convergence and recovery (D44)', '# Step 5.5 — Install the host-updater self-care effector (#185 Drop 2)')
            # D44 changes only the updater check inside --verify from a removed
            # PathExists watcher to its installed periodic timer. Keep checking
            # every surrounding non-updater verify byte against the old flow.
            start = '  # The periodic signed-release poll'
            old_start = '  # #185 self-care — the host-updater effector'
            marker = start if start in text else old_start
            a = text.index(marker); b = text.index('  exit 0\n', a)
            text = text[:a]+text[b:]
            regions=[('if [[ -n "$INSTALL_INTERFACE$INSTALL_LATITUDE$INSTALL_LONGITUDE" ]]', '# Service identity'),
                     # Lock-on migration is independently exercised against
                     # exact-byte/custom-value fixtures in lock-on/test-contract.py.
                     ('# Remove only the exact old template pair;', '# Step 1b — immutable image pull'),
                     # D44 revival replaces only this explicitly registered
                     # updater region; package tests execute its replacement.
                     ('# Step 5.5 — Install ', '# Step 6 — image already verified'),
                     ('# --- config.yaml: generate', '# --- creds:'),
                     ('# BLE ownership is established', '# NOTE: do NOT `systemctl enable`'),
                     # P0-4 rules/inventory are independently rendered and checked.
                     ('# Verify USB hardware visible', '# Capture interface declared'),
                     ('# Verbatim aviary hardened block.', '# Nordic Semiconductor nRF52 Connectivity (BLE)'),
                     # brrdfeeder-naming 2026-09-25: Step 4 renamed the journald drop-in
                     # (99-brrdfeeder.conf, marker-gated legacy migration) and its tier wording.
                     # Behavioural proof: Component/brrdfeeder/install/tests/brrdfeeder-naming/container-tests.py.
                     ('ok "udev rules applied"', '# Step 5 — Install rootful Quadlet at /etc/containers/systemd/'),
                     ('gate quadlets ', 'QUADLET_DIR='),
                     ('# script pick up Quadlet changes.', 'run console_run systemctl --user daemon-reload')]
            for start,end in regions:
                if start.startswith('# BLE ownership') and start not in text:
                    continue  # P0-6 new block, separately exercised by test-ble.py
                a=text.index(start); b=text.index(end,a)
                text=text[:a]+text[b:]
            for line in ['ExecStartPre=${GPS_SEED}\n','TimeoutStartSec=infinity\n','NotifyAccess=all\n','GPS_WAITING=0\n']:
                text=text.replace(line,'')
            text=text.replace('( $entry == "$directory/status.json" || $entry == "$directory/startup.json" )', '$entry == "$directory/status.json"')
            # P0 tests independently execute both helpers and the rendered Quadlet.
            for name in ['enrollment_display_available', 'console_memory_policy']:
                marker=name+'() {'
                if marker in text:
                    a=text.index(marker); b=text.index('\n}\n\n',a)+4
                    text=text[:a]+text[b:]
            text=text.replace('elif enrollment_display_available; then', 'elif [[ -t 0 ]]; then')
            text=text.replace('no visible terminal for Device Flow.', 'no interactive terminal for Device Flow.')
            text=text.replace('\nconsole_memory_policy\n', '\n')
            text=text.replace('PodmanArgs=--pids-limit=64${CONSOLE_MEMORY_ARGS}', 'PodmanArgs=--pids-limit=64 --memory=96m --memory-swap=96m')
            text='\n'.join(line for line in text.split('\n') if not line.startswith(('say "GPS placement matters:', 'say "POSITION on the console')))
            text=text.replace('say "Legacy Nordic serial BLE adapter not detected; this does not test the Realtek USB Bluetooth receiver used by rid_ble."',
                              'warn "Nordic nRF52 not detected (optional — BLE sensor)"')
            # brrdfeeder-naming 2026-09-25: the status readout recognizes both the
            # current and the legacy drop-in name; registered for this packet.
            text=text.replace('''  if [[ -f "$JOURNALD_DROPIN" ]]; then
    ok "journald drop-in present (BRRDfeeder hardening installed)"
  elif [[ -f "$JOURNALD_DROPIN_LEGACY" ]]; then
    warn "legacy journald drop-in present — re-run the installer to migrate it to $JOURNALD_DROPIN"
  else
    say "journald drop-in absent (persistent storage_class — disk journal)"
  fi''','''  [[ -f "$JOURNALD_DROPIN" ]]   && ok "journald drop-in present (Open hardening installed)" \\
                                || say "journald drop-in absent (persistent storage_class — disk journal)"''')
            # Naming packet: comment-only product name in the generated Quadlet.
            text=text.replace('canonical BRRDfeeder platform is Pi OS / Ubuntu.',
                              'canonical Open platform is Pi OS / Ubuntu.')
            # Generated comments are explicitly in the vocabulary-cleanup scope.
            return '\n'.join(line for line in text.splitlines() if not line.lstrip().startswith('#'))
        self.assertEqual(normalize(SOURCE).rstrip('\n'),old)

    def test_order(self):
        self.assertLess(HELPER.index('stop_unit system brrdfeeder-updater.path'),HELPER.index('stop_unit system brrdfeeder-engine.service'))
        self.assertLess(HELPER.index('stop_unit user brrdhouse.service'),HELPER.index('remove_file /etc/brrdfeeder/brrdhouse.container'))
        self.assertLess(HELPER.index('act user_command systemctl --user daemon-reload'),HELPER.index('act userdel'))
        self.assertLess(HELPER.index('remove_images user '),HELPER.index('act loginctl disable-linger'))

    def test_legacy_adoption_is_not_an_identity_bypass(self):
        self.assertLess(HELPER.index('$id -ge 100'),HELPER.index('if (( adopt ))'))
        self.assertIn('account receipt mismatch',HELPER)
        self.assertIn('EXPLICIT LEGACY ADOPTION',HELPER)

if __name__=='__main__': unittest.main(verbosity=2)
