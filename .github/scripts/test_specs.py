#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Negative controls for the public specification gate, with isolated copies."""

from copy import deepcopy
from pathlib import Path
import shutil
import tempfile
import unittest

import specs


class ClaimGate(unittest.TestCase):
    def setUp(self):
        self.data = specs.load(specs.ROOT / "docs/specs/claims.yaml")

    def rejects(self, data, message):
        with self.assertRaisesRegex(ValueError, message):
            specs.validate(data)

    def test_registry_and_local_links(self):
        specs.validate(self.data)
        specs.check_links(specs.ROOT)

    def test_empty_and_duplicate_claims(self):
        empty = deepcopy(self.data)
        empty["claims"] = []
        self.rejects(empty, "zero claims")
        self.data["claims"].append(deepcopy(self.data["claims"][0]))
        self.rejects(self.data, "duplicate claim")

    def test_unscoped_and_unknown_status(self):
        for field, value, message in [("scope", "", "missing scope"),
                                      ("scope", "all", "unscoped claim"),
                                      ("status", "implemented", "invalid status"),
                                      ("evidence_kind", "assumed", "invalid evidence kind")]:
            with self.subTest(field=field, value=value):
                data = deepcopy(self.data)
                data["claims"][0][field] = value
                self.rejects(data, message)

    def test_descriptive_claim_ids(self):
        evidence = [c for c in self.data["claims"] if c["section"] == "evidence"]
        specs.validate(self.data)
        rendered = specs.table(evidence)
        for c in evidence:
            self.assertIn(f"| **{c['title']}** |", rendered)
            self.assertNotIn(c["id"], rendered)
        self.data["claims"].append(deepcopy(evidence[0]))
        self.rejects(self.data, "duplicate claim")

    def test_invalid_claim_ids(self):
        for cid in (None, 1, "", " ", "bad id", "-claim", "claim-", "claim--name", "Claim-name"):
            with self.subTest(claim=cid):
                data = deepcopy(self.data)
                data["claims"][0]["id"] = cid
                self.rejects(data, "invalid claim ID")

    def test_missing_unit_and_owner(self):
        for field in ("unit", "owner"):
            with self.subTest(field=field):
                data = deepcopy(self.data)
                data["claims"][0][field] = ""
                self.rejects(data, f"missing {field}")

    def test_missing_or_mutable_source(self):
        for sources in ([], ["https://github.com/cybrrd/brrdfeeder/blob/main/README.md"],
                        ["evidence/missing.md"], ["evidence/design-basis.md#missing"],
                        ["http://example.invalid/receipt"], ["../../SECURITY.md"]):
            with self.subTest(sources=sources):
                data = deepcopy(self.data)
                data["claims"][0]["source_ref"] = sources
                with self.assertRaises(ValueError):
                    specs.validate(data)

    def test_no_measurement_without_receipt(self):
        self.data["claims"][0]["status"] = "measured"
        self.rejects(self.data, "executed receipt")
        self.data["claims"][0]["verification"]["receipt"] = {"ref": "evidence/design-basis.md"}
        self.rejects(self.data, "executed receipt")

    def test_receipt_metadata_and_nonzero_denominator(self):
        c = self.data["claims"][0]
        c["status"] = "measured"
        c["evidence_kind"] = "synthetic"
        # Format-only synthetic test input, never published as product evidence.
        receipt = {"ref": "https://github.com/cybrrd/brrdfeeder/actions/runs/123",
                   "source_sha": "a" * 40, "date": "2026-10-26", "environment": "synthetic unit test",
                   "denominator": 1, "result": "format example only"}
        c["verification"]["receipt"] = receipt
        specs.validate(self.data)
        for field, value, message in [("denominator", 0, "denominator"),
                                      ("environment", "", "environment/result"),
                                      ("result", "", "environment/result"),
                                      ("source_sha", "main", "source SHA"),
                                      ("ref", "evidence/design-basis.md", "not an executed receipt")]:
            with self.subTest(field=field):
                data = deepcopy(self.data)
                data["claims"][0]["verification"]["receipt"][field] = value
                self.rejects(data, message)

    def test_target_requires_scenario_and_value(self):
        self.data["claims"][0]["verification"]["scenario"] = ""
        self.rejects(self.data, "acceptance scenario")
        self.data = specs.load(specs.ROOT / "docs/specs/claims.yaml")
        self.data["claims"][0]["value"] = None
        self.rejects(self.data, "unset target")

    def test_service_requires_miss_and_signal(self):
        c = next(c for c in self.data["claims"] if c["section"] == "service")
        del c["failure_signal"]
        self.rejects(self.data, "miss behavior or visible signal")

    def test_capacity_is_advisory(self):
        for cid in ("P01", "P02", "P03", "D13", "D14"):
            with self.subTest(claim=cid):
                data = deepcopy(self.data)
                next(c for c in data["claims"] if c["id"] == cid)["semantics"] = "expectation"
                self.rejects(data, "capacity must be advisory")

    def test_explicit_public_review(self):
        self.data["claims"][0]["public"] = False
        self.rejects(self.data, "non-public claim")
        self.data["claims"][0]["public"] = True
        self.data["claims"][0]["sanitization_review"] = ""
        self.rejects(self.data, "sanitization_review")

    def test_duplicate_keys_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            specs.no_duplicates([("scope", "0.9-beta"), ("scope", "future-v5")])

    def test_metadata_tag_mismatch(self):
        self.data["specification"]["release_tag"] = "v0.8.31"
        self.rejects(self.data, "tag/version mismatch")

    def test_release_tag_binding(self):
        self.data["specification"]["release_tag"] = "v0.9.0"
        specs.validate(self.data)
        with tempfile.TemporaryDirectory(prefix="specs-release-") as tmp:
            root = Path(tmp)
            shutil.copytree(specs.ROOT / "docs/specs", root / "docs/specs")
            specs.update_pages(self.data, root, check=False)
            specs.update_pages(self.data, root, check=True)
            for name in specs.PAGES:
                text = (root / "docs/specs" / name).read_text()
                self.assertIn("Release tag: `v0.9.0`.", text)
                self.assertNotIn("release tag pending", text)

    def test_stale_views_and_missing_markers(self):
        with tempfile.TemporaryDirectory(prefix="specs-gate-") as tmp:
            root = Path(tmp)
            shutil.copytree(specs.ROOT / "docs/specs", root / "docs/specs")
            specs.update_pages(self.data, root, check=True)
            changed = deepcopy(self.data)
            next(c for c in changed["claims"] if c["id"] == "D07")["value"] = 3
            with self.assertRaisesRegex(ValueError, "stale generated content: README.md, OPERATIONS.md"):
                specs.update_pages(changed, root, check=True)
            specs.update_pages(changed, root, check=False)
            specs.update_pages(changed, root, check=True)
            # Every page is checked, including metadata-only changes.
            for name in specs.PAGES:
                path = root / "docs/specs" / name
                original = path.read_text()
                path.write_text(original.replace("Specification revision:", "Altered revision:", 1))
                with self.assertRaisesRegex(ValueError, "stale generated content"):
                    specs.update_pages(changed, root, check=True)
                path.write_text(original)
            path = root / "docs/specs/README.md"
            path.write_text(path.read_text().replace("<!-- BEGIN GENERATED: summary -->", ""))
            with self.assertRaisesRegex(ValueError, "generated blocks"):
                specs.update_pages(changed, root, check=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
