#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Release approval and public scaffold contracts; no network or credentials."""
from pathlib import Path
import re
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[5]

def workflow(name):
    return yaml.safe_load((ROOT/'.github/workflows'/name).read_text())

class Scaffold(unittest.TestCase):
    def test_main_dispatch_builds_every_release_artifact_but_cannot_publish(self):
        jobs = workflow('release.yml')['jobs']
        for name in ('images', 'host-updater'):
            condition = jobs[name]['if']
            self.assertIn("startsWith(github.ref, 'refs/tags/v')", condition, name)
            self.assertIn("github.event_name == 'workflow_dispatch'", condition, name)
            self.assertIn("github.ref == 'refs/heads/main'", condition, name)
            self.assertNotIn('environment', jobs[name], name)
        publisher = jobs['publish-sign']
        self.assertIn("startsWith(github.ref, 'refs/tags/v')", publisher['if'])
        self.assertNotIn('workflow_dispatch', publisher['if'])
        self.assertNotIn('refs/heads/main', publisher['if'])
        self.assertEqual(publisher['environment'], 'release')
        self.assertEqual([name for name, job in jobs.items() if 'environment' in job],
                         ['publish-sign'])

    def test_python_runtime_is_pinned_in_both_release_phases(self):
        for name in ('images', 'publish-sign'):
            steps = workflow('release.yml')['jobs'][name]['steps']
            setups = [s for s in steps if s.get('uses', '').startswith('actions/setup-python@')]
            self.assertEqual(len(setups), 1, name)
            self.assertEqual(setups[0]['with']['python-version'], '3.13.5')

    def test_publisher_requires_environment_and_all_builds(self):
        jobs = workflow('release.yml')['jobs']
        self.assertIn('publish-sign', jobs, 'missing separate approval-gated publisher')
        job = jobs['publish-sign']
        self.assertEqual(job['environment'], 'release')
        self.assertEqual(set(job['needs']), {'gate', 'images', 'host-updater'})
        self.assertIn("vars.RELEASE_APPROVAL_CONFIGURED == 'true'", job['if'])
        self.assertIn("github.repository == 'cybrrd/brrdfeeder'", job['if'])
        self.assertIn("startsWith(github.ref, 'refs/tags/v')", job['if'])

    def test_no_build_or_scan_job_has_registry_or_oidc_writes(self):
        for file in (ROOT/'.github/workflows').glob('*.yml'):
            value = yaml.safe_load(file.read_text())
            self.assertNotIn('id-token', value.get('permissions', {}), file.name)
            for name, job in value['jobs'].items():
                perms = job.get('permissions', value.get('permissions', {}))
                if file.name == 'release.yml' and name == 'publish-sign':
                    self.assertEqual(perms['id-token'], 'write')
                    self.assertEqual(perms['attestations'], 'write')
                else:
                    self.assertNotEqual(perms.get('id-token'), 'write', (file.name, name))
                    self.assertNotEqual(perms.get('packages'), 'write', (file.name, name))
                    self.assertNotEqual(perms.get('contents'), 'write', (file.name, name))

    def test_every_external_action_is_commit_pinned(self):
        for file in (ROOT/'.github/workflows').glob('*.yml'):
            for action in re.findall(r'uses:\s+([^\s]+)', file.read_text()):
                if action.startswith('./'):
                    self.assertTrue((ROOT/action).is_file(), action)
                else:
                    self.assertRegex(action, r'^[\w.-]+/[\w./-]+@[0-9a-f]{40}$', file.name)

    def test_two_image_provenances_are_after_approval(self):
        jobs = workflow('release.yml')['jobs']
        calls = [(name, step) for name, job in jobs.items() for step in job.get('steps', [])
                 if step.get('uses', '').startswith('actions/attest-build-provenance@')]
        self.assertEqual(len(calls), 2)
        self.assertEqual({name for name, _ in calls}, {'publish-sign'})
        self.assertEqual({s['with']['subject-name'] for _, s in calls},
                         {'ghcr.io/cybrrd/brrdfeeder', 'ghcr.io/cybrrd/brrdhouse'})
        for _, step in calls:
            self.assertIn('steps.publish.outputs.', step['with']['subject-digest'])
            self.assertEqual(str(step['with']['push-to-registry']).lower(), 'false')
            self.assertEqual(str(step['with']['create-storage-record']).lower(), 'false')
            self.assertEqual(step['with']['github-token'], '${{ secrets.GITHUB_TOKEN }}')

    def test_attestation_destination_has_the_credentials_or_storage_it_needs(self):
        text = (ROOT/'.github/workflows/release.yml').read_text()
        steps = workflow('release.yml')['jobs']['publish-sign']['steps']
        attestations = [step for step in steps
                        if step.get('uses', '').startswith('actions/attest-build-provenance@')]
        home_registry_auth = '.docker/config.json' in text
        for step in attestations:
            registry = str(step['with'].get('push-to-registry', False)).lower() == 'true'
            storage_record = str(step['with'].get('create-storage-record', True)).lower() == 'true'
            self.assertTrue(
                (registry and home_registry_auth) or (not registry and not storage_record),
                'registry attachment needs ~/.docker/config.json; API-only attestation '
                'must not request the registry-only storage record',
            )

    def test_every_post_publish_step_has_inputs_permissions_and_failure_order(self):
        job = workflow('release.yml')['jobs']['publish-sign']
        self.assertEqual(job['permissions'], {
            'contents': 'write', 'packages': 'write', 'id-token': 'write',
            'attestations': 'write',
        })
        steps = job['steps']
        by_name = {step.get('name'): (index, step) for index, step in enumerate(steps)}
        publish_index = next(index for index, step in enumerate(steps)
                             if step.get('id') == 'publish')
        engine_index, engine = by_name['Attest engine provenance']
        console_index, console = by_name['Attest console provenance']
        draft_index, draft = by_name[
            'Create draft release with changelog, digests, SBOMs and attestation URLs']
        receipt_index, receipt = by_name['Retain approved release receipts']
        cleanup_index, cleanup = by_name['Remove ephemeral publisher registry credentials']
        self.assertLess(publish_index, engine_index)
        self.assertLess(engine_index, console_index)
        self.assertLess(console_index, draft_index)
        self.assertLess(draft_index, receipt_index)
        self.assertEqual(cleanup_index, len(steps)-1)

        self.assertEqual(engine['id'], 'engine-provenance')
        self.assertEqual(console['id'], 'console-provenance')
        self.assertEqual(engine['with']['subject-digest'],
                         '${{ steps.publish.outputs.engine-digest }}')
        self.assertEqual(console['with']['subject-digest'],
                         '${{ steps.publish.outputs.console-digest }}')
        self.assertEqual(draft['run'], 'python3 .github/scripts/draft-release.py')
        self.assertEqual(draft['env'], {
            'GH_TOKEN': '${{ secrets.GITHUB_TOKEN }}',
            'ENGINE_ATTESTATION_URL':
                '${{ steps.engine-provenance.outputs.attestation-url }}',
            'CONSOLE_ATTESTATION_URL':
                '${{ steps.console-provenance.outputs.attestation-url }}',
        })
        self.assertEqual(receipt['with'], {
            'name': 'approved-release-receipts',
            'path': '${{ runner.temp }}/release-notes/',
            'if-no-files-found': 'error',
            'retention-days': 7,
        })
        self.assertEqual(cleanup['if'], 'always()')
        self.assertIn('"$RUNNER_TEMP/registry-auth/config.json"', cleanup['run'])
        self.assertNotIn('~/.docker', text := (ROOT/'.github/workflows/release.yml').read_text())
        self.assertNotIn('$GITHUB_ENV', text)

    def test_native_builds_and_stable_full_gate(self):
        release = workflow('release.yml')
        self.assertEqual(release['jobs']['images']['runs-on'], 'ubuntu-24.04-arm')
        gate = workflow('test.yml')
        self.assertEqual(gate['name'], 'Public offline contracts')
        self.assertIn('contracts', gate['jobs'])
        text = (ROOT/'.github/workflows/test.yml').read_text()
        self.assertIn('run.py --group all', text)
        self.assertIn('pull_request:', text)
        self.assertIn('branches: [main]', text)

    def test_security_scanners_and_weekly_updates(self):
        self.assertTrue((ROOT/'.github/workflows/codeql.yml').exists(), 'CodeQL absent')
        self.assertTrue((ROOT/'.github/workflows/scorecard.yml').exists(), 'Scorecard absent')
        codeql = workflow('codeql.yml')
        languages = {x['language'] for x in codeql['jobs']['analyze']['strategy']['matrix']['include']}
        self.assertEqual(languages, {'go', 'python', 'rust'})
        score = (ROOT/'.github/workflows/scorecard.yml').read_text()
        self.assertIn('publish_results: false', score)
        self.assertIn('github/codeql-action/upload-sarif@', score)
        dependabot = yaml.safe_load((ROOT/'.github/dependabot.yml').read_text())
        self.assertEqual({u['package-ecosystem'] for u in dependabot['updates']},
                         {'cargo', 'gomod', 'github-actions', 'docker'})
        for update in dependabot['updates']:
            self.assertEqual(update['schedule']['interval'], 'weekly')
            self.assertTrue(update['groups'])
            self.assertTrue(any(g.get('applies-to') == 'security-updates' for g in update['groups'].values()))

    def test_conservative_contribution_and_security_policy(self):
        for name in ['README.md', 'SECURITY.md', 'CONTRIBUTING.md',
                     '.github/CODEOWNERS', '.github/pull_request_template.md']:
            self.assertTrue((ROOT/name).is_file(), name)
        policy = (ROOT/'CONTRIBUTING.md').read_text()
        self.assertIn('not accepted', policy)
        # Preserve the repository owner's policy at public base 83fc7b0.
        # Its legal consistency is reviewed by the owner, not rewritten by CI.
        self.assertIn('currently inoperative as contributions are closed', policy)
        self.assertIn('will require active electronic signature', policy)
        self.assertIn('before', policy.lower())
        self.assertIn('* @cybrrd', (ROOT/'.github/CODEOWNERS').read_text())
        self.assertIn('/security/advisories/new', (ROOT/'SECURITY.md').read_text())
        self.assertIn('Scorecard', (ROOT/'README.md').read_text())

if __name__ == '__main__':
    unittest.main(verbosity=2)
