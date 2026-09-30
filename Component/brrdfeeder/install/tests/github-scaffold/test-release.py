#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Behavioral approval/artifact boundary checks with inert publication tools."""
import json
from pathlib import Path
import unittest
from release_fixture import ReleaseFixture

class Release(unittest.TestCase):
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
    def test_missing_approval_acknowledgement_refuses_all_effectors(self):
        with ReleaseFixture() as f:
            f.build_both()
            p=f.run('publish-images.sh',RELEASE_APPROVAL_CONFIGURED='')
            self.assertNotEqual(p.returncode,0)
            self.assertFalse(f.writes())
    def test_branch_dispatch_and_moved_tag_refuse(self):
        for overrides in [{'GITHUB_REF':'refs/heads/main','GITHUB_REF_NAME':'main'},
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
            self.assertIn('Build sequence: 1001',(f.work/'out/release-notes/notes.md').read_text())
    def test_invalid_attestation_url_or_host_pin_prevents_draft(self):
        for wrong_host in (True,False):
            with self.subTest(wrong_host=wrong_host), ReleaseFixture() as f:
                f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
                env={}
                if wrong_host: (f.work/'out/host-updater/brrdfeeder-release-arm64').write_bytes(b'corrupt')
                else: env['ENGINE_ATTESTATION_URL']='https://evil.example/123'
                self.assertNotEqual(f.run('draft-release.py',**env).returncode,0)
                self.assertFalse(any(name=='gh' for name,_ in f.calls()))
    def test_existing_release_is_not_clobbered(self):
        with ReleaseFixture() as f:
            f.build_both();self.assertEqual(f.run('publish-images.sh').returncode,0)
            self.assertNotEqual(f.run('draft-release.py',FIXTURE_EXISTING_RELEASE='1').returncode,0)
            calls=[args for name,args in f.calls() if name=='gh']
            self.assertEqual(len(calls),1)
            self.assertNotIn('--clobber',calls[0])

if __name__=='__main__': unittest.main(verbosity=2)
