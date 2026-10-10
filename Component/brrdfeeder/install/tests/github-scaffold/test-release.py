#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Behavioral approval/artifact boundary checks with inert publication tools."""
import json
import hashlib
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from release_fixture import ReleaseFixture

class Release(unittest.TestCase):
    def test_signed_receipt_binds_same_run_and_every_asset(self):
        with ReleaseFixture() as f:
            f.build_both()
            self.assertEqual(f.run('publish-images.sh').returncode, 0)
            p = f.run('draft-release.py')
            self.assertEqual(p.returncode, 0, p.stderr)
            out = f.work/'out/release-notes'
            self.assertTrue((out/'release-receipt.json').is_file(), 'missing signed receipt')
            receipt = json.loads((out/'release-receipt.json').read_text())
            self.assertEqual(receipt['run_id'], 12345)
            self.assertEqual(receipt['run_attempt'], 1)
            self.assertEqual(receipt['revision'], f.env['GITHUB_SHA'])
            self.assertEqual(receipt['repository'], 'cybrrd/brrdfeeder')
            self.assertEqual(receipt['workflow_ref'], f.env['GITHUB_WORKFLOW_REF'])
            self.assertEqual(set(receipt['images']), {'engine', 'console'})
            self.assertEqual(len(receipt['assets']), 10)
            for name, digest in receipt['assets'].items():
                self.assertEqual(hashlib.sha256((out/name).read_bytes()).hexdigest(), digest)
            calls = f.calls()
            signing = next(i for i,(name,args) in enumerate(calls)
                           if name == 'cosign' and args[0] == 'sign-blob')
            draft = next(i for i,(name,args) in enumerate(calls) if name == 'gh')
            self.assertLess(signing, draft)
            self.assertIn('--new-bundle-format', calls[signing][1])
            for name in ('release-receipt.json', 'release-receipt.sigstore.json'):
                self.assertTrue(any(arg.endswith('/'+name) for arg in calls[draft][1]))

    def test_receipt_sign_failure_never_creates_draft(self):
        with ReleaseFixture() as f:
            f.build_both()
            self.assertEqual(f.run('publish-images.sh').returncode, 0)
            p = f.run('draft-release.py', FIXTURE_RECEIPT_SIGN_FAIL='1')
            self.assertNotEqual(p.returncode, 0, 'receipt signing failure ignored')
            self.assertFalse(any(name == 'gh' for name,_ in f.calls()))

    def test_receipt_rejects_wrong_run_attempt_workflow_and_event(self):
        for env in ({'GITHUB_RUN_ID':'23456'}, {'GITHUB_RUN_ATTEMPT':'2'},
                    {'GITHUB_WORKFLOW_REF':'fork/repo/.github/workflows/release.yml@refs/tags/v1.2.3'},
                    {'GITHUB_EVENT_NAME':'pull_request'}):
            with self.subTest(env=env), ReleaseFixture() as f:
                f.build_both()
                self.assertEqual(f.run('publish-images.sh').returncode, 0)
                p = f.run('draft-release.py', **env)
                self.assertNotEqual(p.returncode, 0, 'unbound run input accepted')
                self.assertFalse(any(name == 'gh' for name,_ in f.calls()))

    def test_fixture_is_independent_of_host_ci_environment(self):
        explicit = {
            'GITHUB_ACTOR', 'GITHUB_EVENT_NAME', 'GITHUB_OUTPUT',
            'GITHUB_REF', 'GITHUB_REF_NAME', 'GITHUB_REPOSITORY',
            'GITHUB_SHA', 'GITHUB_STEP_SUMMARY', 'RUNNER_TEMP',
            'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_WORKFLOW_REF',
        }
        for host_event in ('workflow_dispatch', 'push'):
            host = {
                'GITHUB_EVENT_NAME': host_event,
                'GITHUB_ENV': '/host/github-env',
                'GITHUB_WORKSPACE': '/host/workspace',
                'RUNNER_OS': 'host-runner',
                'ACTIONS_ID_TOKEN_REQUEST_URL': 'https://host.invalid/token',
            }
            with self.subTest(host_event=host_event), patch.dict(os.environ, host):
                with ReleaseFixture() as f:
                    self.assertEqual(f.env['GITHUB_EVENT_NAME'], 'push')
                    leaked = {name for name in f.env
                              if name.startswith(('GITHUB_', 'RUNNER_', 'ACTIONS_'))
                              and name not in explicit}
                    self.assertEqual(leaked, set())
                    result = f.build('engine', GITHUB_REF='refs/heads/main',
                                     GITHUB_REF_NAME='main')
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(f.calls())

    def test_main_dispatch_validates_version_and_builds_without_a_tag(self):
        with ReleaseFixture() as f:
            result = f.build('engine', GITHUB_EVENT_NAME='workflow_dispatch',
                             GITHUB_REF='refs/heads/main', GITHUB_REF_NAME='main')
            self.assertEqual(result.returncode, 0, result.stderr)
            metadata = json.loads((f.work/'out/release/engine/metadata.json').read_text())
            self.assertIsNone(metadata['tag'])
            self.assertEqual(metadata['product_version'], '1.2.3')
            self.assertFalse(f.writes())

    def test_main_dispatch_refuses_an_invalid_untagged_version(self):
        with ReleaseFixture() as f:
            (f.work/'Component/aviary/engine/Cargo.toml').write_text(
                '[package]\nname = "engine"\nversion = "not-a-version"\n')
            result = f.build('engine', GITHUB_EVENT_NAME='workflow_dispatch',
                             GITHUB_REF='refs/heads/main', GITHUB_REF_NAME='main')
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(f.calls(), 'invalid version reached a build effector')

    def test_release_tag_must_match_engine_product_version(self):
        cases = [
            ('v0.8.20', '0.2.0', False),
            ('v0.8.21', '0.8.20', False),
            ('v0.8.20', '0.8.20', True),
        ]
        for tag, engine_version, accepted in cases:
            with self.subTest(tag=tag, engine_version=engine_version), ReleaseFixture() as f:
                subprocess.run(['git', 'tag', tag], cwd=f.work, env=f.env,
                               check=True, capture_output=True)
                manifest = f.work/'Component/aviary/engine/Cargo.toml'
                manifest.write_text('[package]\nname = "engine"\nversion = "'+engine_version+'"\n')
                result = f.build('engine', GITHUB_REF='refs/tags/'+tag,
                                 GITHUB_REF_NAME=tag)
                self.assertEqual(result.returncode == 0, accepted, result.stderr)
                if not accepted:
                    self.assertFalse(f.calls(), 'mismatch reached a build effector')

    def test_native_console_build_and_no_build_phase_writes(self):
        with ReleaseFixture() as f:
            f.build_both()
            pulls=[args for name,args in f.calls() if name=='podman' and args[0]=='pull']
            self.assertEqual(len(pulls),1)
            self.assertIn('linux/arm64',pulls[0])
            self.assertFalse(f.writes())
            for component in ('engine','console'):
                meta=json.loads((f.work/'out/release'/component/'metadata.json').read_text())
                self.assertEqual(meta['build_seq'],1001)
                self.assertEqual(meta['product_version'],'1.2.3')
    def test_missing_approval_acknowledgement_refuses_all_effectors(self):
        with ReleaseFixture() as f:
            f.build_both()
            p=f.run('publish-images.sh',RELEASE_APPROVAL_CONFIGURED='')
            self.assertNotEqual(p.returncode,0)
            self.assertFalse(f.writes())
    def test_other_branch_or_non_dispatch_main_and_moved_tag_refuse(self):
        for overrides in [{'GITHUB_REF':'refs/heads/main','GITHUB_REF_NAME':'main'},
                          {'GITHUB_EVENT_NAME':'workflow_dispatch',
                           'GITHUB_REF':'refs/heads/topic','GITHUB_REF_NAME':'topic'},
                          {'GITHUB_SHA':'a'*40},{'GITHUB_REF_NAME':'v9','GITHUB_REF':'refs/tags/v9'}]:
            with self.subTest(overrides=overrides), ReleaseFixture() as f:
                p=f.build('engine',**overrides)
                self.assertNotEqual(p.returncode,0)
                self.assertFalse(f.calls())
    def test_wrong_architecture_revision_or_sequence_refuses(self):
        for overrides in [{'FIXTURE_ARCH':'amd64'},{'FIXTURE_REVISION':'a'*40},{'FIXTURE_SEQUENCE':'1'}]:
            with self.subTest(overrides=overrides), ReleaseFixture() as f:
                p=f.build('engine',**overrides)
                self.assertNotEqual(p.returncode,0)
                self.assertFalse(f.writes())
    def test_either_archive_or_sbom_drift_refuses_before_login(self):
        for component in ('engine','console'):
            for name in ('image.oci.tar','sbom.cdx.json','metadata.json'):
                with self.subTest(component=component,name=name), ReleaseFixture() as f:
                    f.build_both()
                    path=f.work/'out/release'/component/name
                    path.write_text('corrupt')
                    p=f.run('publish-images.sh')
                    self.assertNotEqual(p.returncode,0)
                    self.assertFalse(f.writes())
    def test_symlink_or_missing_artifact_refuses(self):
        for symlink in (True,False):
            with self.subTest(symlink=symlink), ReleaseFixture() as f:
                f.build_both()
                path=f.work/'out/release/console/image.oci.tar';path.unlink()
                if symlink: path.symlink_to(f.work/'out/release/engine/image.oci.tar')
                self.assertNotEqual(f.run('publish-images.sh').returncode,0)
                self.assertFalse(f.writes())
    def test_forged_repository_in_metadata_refuses(self):
        with ReleaseFixture() as f:
            f.build_both()
            path=f.work/'out/release/engine/metadata.json'
            data=json.loads(path.read_text());data['image']='ghcr.io/foreign/image'
            path.write_text(json.dumps(data))
            self.assertNotEqual(f.run('publish-images.sh').returncode,0)
            self.assertFalse(f.writes())
    def test_host_pin_mismatch_prevents_any_registry_operation(self):
        with ReleaseFixture() as f:
            f.build_both()
            (f.work/'out/host-updater/brrdfeeder-release-arm64').write_bytes(b'corrupt')
            self.assertNotEqual(f.run('publish-images.sh').returncode,0)
            self.assertFalse(f.writes())
    def test_publisher_never_builds_or_regenerates_sbom(self):
        with ReleaseFixture() as f:
            f.build_both();before=len(f.calls())
            p=f.run('publish-images.sh')
            self.assertEqual(p.returncode,0,p.stderr)
            self.assertFalse(any(name in ('podman','syft') for name,_ in f.calls()[before:]))
            self.assertIn('engine-digest=sha256:'+'b'*64,(f.work/'outputs').read_text())
            self.assertIn('console-digest=sha256:'+'c'*64,(f.work/'outputs').read_text())
    def test_remote_digest_mismatch_never_signs(self):
        with ReleaseFixture() as f:
            f.build_both()
            p=f.run('publish-images.sh',FIXTURE_REMOTE_DIGEST='sha256:'+'a'*64)
            self.assertNotEqual(p.returncode,0)
            self.assertFalse(any(name=='cosign' for name,_ in f.calls()))
    def test_repeated_publisher_uses_existing_packages_by_tag_and_exact_digest(self):
        with ReleaseFixture() as f:
            f.build_both()
            for _ in range(2):
                p=f.run('publish-images.sh')
                self.assertEqual(p.returncode,0,p.stderr)
            copies=[args for name,args in f.calls() if name=='skopeo' and args[0]=='copy']
            self.assertEqual(len(copies),4)
            destinations=[args[-1] for args in copies]
            expected={
                'docker://ghcr.io/cybrrd/brrdfeeder:v1.2.3',
                'docker://ghcr.io/cybrrd/brrdhouse:v1.2.3',
            }
            self.assertEqual(set(destinations),expected)
            self.assertTrue(all(':latest' not in destination for destination in destinations))
            signatures=[args[-1] for name,args in f.calls() if name=='cosign']
            self.assertEqual(len(signatures),4)
            self.assertEqual(set(signatures),{
                'ghcr.io/cybrrd/brrdfeeder@sha256:'+'b'*64,
                'ghcr.io/cybrrd/brrdhouse@sha256:'+'c'*64,
            })
    def test_draft_has_changelog_sboms_digests_and_attestations_never_publish(self):
        with ReleaseFixture() as f:
            f.build_both()
            self.assertEqual(f.run('publish-images.sh').returncode,0)
            p=f.run('draft-release.py');self.assertEqual(p.returncode,0,p.stderr)
            args=next(args for name,args in f.calls() if name=='gh')
            for flag in ('--draft','--verify-tag','--generate-notes'): self.assertIn(flag,args)
            for name in ('engine-sbom.cdx.json','console-sbom.cdx.json','digests.txt','attestations.json','SHA256SUMS'):
                self.assertTrue(any(arg.endswith('/'+name) for arg in args),name)
            self.assertNotIn('edit',args)
            notes=(f.work/'out/release-notes/notes.md').read_text()
            self.assertIn('Product version: 1.2.3',notes)
            self.assertIn('Build sequence: 1001',notes)
    def test_invalid_attestation_url_or_host_pin_prevents_draft(self):
        for wrong_host in (True,False):
            with self.subTest(wrong_host=wrong_host), ReleaseFixture() as f:
                f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
                env={}
                if wrong_host: (f.work/'out/host-updater/brrdfeeder-release-arm64').write_bytes(b'corrupt')
                else: env['ENGINE_ATTESTATION_URL']='https://evil.example/123'
                self.assertNotEqual(f.run('draft-release.py',**env).returncode,0)
                self.assertFalse(any(name=='gh' for name,_ in f.calls()))
    def test_missing_token_or_post_publish_file_prevents_draft(self):
        with ReleaseFixture() as f:
            f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
            self.assertNotEqual(f.run('draft-release.py',GH_TOKEN='').returncode,0)
            self.assertFalse(any(name=='gh' for name,_ in f.calls()))
        with ReleaseFixture() as f:
            f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
            (f.work/'out/release/engine/image.txt').unlink()
            self.assertNotEqual(f.run('draft-release.py').returncode,0)
            self.assertFalse(any(name=='gh' for name,_ in f.calls()))
    def test_existing_release_is_not_clobbered(self):
        with ReleaseFixture() as f:
            f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
            self.assertNotEqual(f.run('draft-release.py',FIXTURE_EXISTING_RELEASE='1').returncode,0)
            calls=[args for name,args in f.calls() if name=='gh']
            self.assertEqual(len(calls),1)
            self.assertNotIn('--clobber',calls[0])

if __name__=='__main__': unittest.main(verbosity=2)
