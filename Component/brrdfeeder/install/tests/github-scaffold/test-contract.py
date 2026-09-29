#!/usr/bin/env python3
"""Release approval and public scaffold contracts; no network or credentials."""
from pathlib import Path
import re
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[5]

def workflow(name):
    return yaml.safe_load((ROOT/'.github/workflows'/name).read_text())

class Scaffold(unittest.TestCase):
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
            self.assertEqual(str(step['with']['push-to-registry']).lower(), 'true')

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
        self.assertIn('CLA TEXT PLACEHOLDER', policy)
        self.assertIn('before', policy.lower())
        self.assertIn('* @cybrrd', (ROOT/'.github/CODEOWNERS').read_text())
        self.assertIn('/security/advisories/new', (ROOT/'SECURITY.md').read_text())
        self.assertIn('Scorecard', (ROOT/'README.md').read_text())

if __name__ == '__main__':
    unittest.main(verbosity=2)
