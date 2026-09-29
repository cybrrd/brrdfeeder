#!/usr/bin/env python3
"""Approval/publication mutants must die by assertions in disposable copies."""
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import release_fixture

HERE=Path(__file__).parent
ROOT=HERE.resolve().parents[4]
def load(name,file):
    spec=importlib.util.spec_from_file_location(name,HERE/file)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module
contracts=load('scaffold_contract','test-contract.py')
behavior=load('scaffold_behavior','test-release.py')
cases=[
    ('environment-bypass','.github/workflows/release.yml','environment: release','environment: unprotected',
     contracts.Scaffold,'test_publisher_requires_environment_and_all_builds'),
    ('oidc-on-build','.github/workflows/release.yml','permissions:\n      contents: read\n    steps:',
     'permissions:\n      contents: read\n      id-token: write\n    steps:',
     contracts.Scaffold,'test_no_build_or_scan_job_has_registry_or_oidc_writes'),
    ('registry-on-build','.github/workflows/release.yml','permissions:\n      contents: read\n    steps:',
     'permissions:\n      contents: read\n      packages: write\n    steps:',
     contracts.Scaffold,'test_no_build_or_scan_job_has_registry_or_oidc_writes'),
    ('settings-bypass','.github/workflows/release.yml'," && vars.RELEASE_APPROVAL_CONFIGURED == 'true'",'',
     contracts.Scaffold,'test_publisher_requires_environment_and_all_builds'),
    ('floating-action','.github/workflows/release.yml','actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683','actions/checkout@v4',
     contracts.Scaffold,'test_every_external_action_is_commit_pinned'),
    ('provenance-missing','.github/workflows/release.yml','actions/attest-build-provenance@','actions/not-provenance@',
     contracts.Scaffold,'test_two_image_provenances_are_after_approval'),
    ('artifact-check-bypass','.github/scripts/release-metadata.py','require(expected == actual,','require(True,',
     behavior.Release,'test_either_archive_or_sbom_drift_refuses_before_login'),
    ('approval-ack-bypass','.github/scripts/publish-images.sh','[[ ${RELEASE_APPROVAL_CONFIGURED:-} == true ]]','[[ true == true ]]',
     behavior.Release,'test_missing_approval_acknowledgement_refuses_all_effectors'),
    ('non-draft-release','.github/scripts/draft-release.py',"'--draft', ",'',
     behavior.Release,'test_draft_has_changelog_sboms_digests_and_attestations_never_publish'),
]
results=[]
for name,relative,old,new,test,method in cases:
    with tempfile.TemporaryDirectory(prefix='scaffold-mutant-') as tmp:
        root=Path(tmp)
        shutil.copytree(ROOT/'.github',root/'.github')
        (root/'Component/brrdhouse').mkdir(parents=True)
        shutil.copyfile(ROOT/'Component/brrdhouse/Containerfile',root/'Component/brrdhouse/Containerfile')
        path=root/relative;source=path.read_text()
        if old not in source: raise AssertionError('stale mutation: '+name)
        path.write_text(source.replace(old,new,1))
        contracts.ROOT=root;release_fixture.ROOT=root
        stream=io.StringIO()
        result=unittest.TextTestRunner(stream=stream,verbosity=2).run(unittest.TestSuite([test(method)]))
        killed=bool(result.failures) and not result.errors
        results.append(dict(name=name,assertion=method,killed=killed))
        if not killed: print(stream.getvalue())
print(json.dumps(results,indent=2))
raise SystemExit(not all(r['killed'] for r in results))
