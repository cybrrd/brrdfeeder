#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Validate the public claim registry and render its Markdown views (stdlib only).

claims.yaml uses JSON-compatible YAML with SPDX comments. This intentionally
accepts that small syntax, not arbitrary YAML tags, aliases or duplicate keys.
The check proves documentation consistency, never product acceptance.
"""

import argparse
from collections import Counter
from datetime import date
import json
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
PAGES = {
    "README.md": ["summary"],
    "REQUIREMENTS.md": ["requirements", "reception"],
    "OPERATIONS.md": ["operations", "enrollment", "outage", "updates", "support"],
    "SERVICE-LEVELS.md": ["service", "service-methods", "capacity", "future", "placeholders"],
    "KNOWN-LIMITS.md": ["beta-limits", "placeholders"],
    "SBOM.md": ["evidence"],
}
SECTIONS = {"requirements", "reception", "operations", "outage", "updates",
            "service", "capacity", "enrollment", "support", "future", "placeholders", "evidence", "beta-limits"}
SUMMARY = {"H07", "D07", "U02", "S01", "S02", "P01", "P02"}
STATUSES = {"measured", "target", "unverified"}
KINDS = {"source", "synthetic", "native", "field"}
SCOPES = {"0.9-beta", "future-v5"}
SEMANTICS = {"expectation", "planning-guidance", "physical-buffer", "advisory-alert"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load(path):
    text = "\n".join(line for line in path.read_text().splitlines()
                     if not line.lstrip().startswith("#"))
    return json.loads(text, object_pairs_hook=no_duplicates)


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def public_ref(ref, root):
    require(nonempty(ref), "missing source/receipt reference")
    url = urlsplit(ref)
    if url.scheme or url.netloc:
        require(url.scheme == "https" and url.hostname == "github.com"
                and url.username is None and url.password is None
                and not url.query and url.port is None,
                f"reference must be a public, credential-free GitHub URL: {ref}")
        prefix = "/cybrrd/brrdfeeder/"
        require(url.path.startswith(prefix), f"unexpected evidence repository: {ref}")
        tail = url.path[len(prefix):]
        require(bool(re.fullmatch(r"blob/[0-9a-f]{40}/[^\s]+", tail)
                     or re.fullmatch(r"actions/runs/[0-9]+(?:/[^\s]+)?", tail)
                     or re.fullmatch(r"releases/(?:tag|download)/v[^/]+(?:/[^\s]+)?", tail)),
                f"reference must pin a source commit, run or release tag: {ref}")
    else:
        require(url.path.startswith("evidence/") and ".." not in Path(url.path).parts,
                f"local evidence must stay in docs/specs/evidence: {ref}")
        path = root / "docs/specs" / url.path
        require(path.is_file(), f"missing local evidence: {ref}")
        if url.fragment:
            require(path.suffix == ".md", f"anchor on non-Markdown evidence: {ref}")
            anchors = {slug(line.lstrip("# ").strip()) for line in path.read_text().splitlines()
                       if line.startswith("#")}
            require(url.fragment in anchors, f"missing evidence heading: {ref}")


def slug(text):
    return re.sub(r"[^\w\- ]", "", text.lower()).replace(" ", "-")


def validate(data, root=ROOT):
    require(isinstance(data, dict) and set(data) == {"specification", "claims"},
            "registry must contain specification and claims")
    meta = data["specification"]
    require(isinstance(meta, dict) and set(meta) == {
        "product_version", "applies_to", "revision", "source_sha", "release_tag"},
        "invalid specification metadata")
    require(meta["product_version"] == "0.9.0" and meta["applies_to"] == "0.9-beta",
            "unscoped or unexpected product specification")
    date.fromisoformat(meta["revision"])
    require(bool(re.fullmatch(r"[0-9a-f]{40}", meta["source_sha"])), "source SHA must be full length")
    require(meta["release_tag"] in {None, "v" + meta["product_version"]}, "release tag/version mismatch")
    claims = data["claims"]
    require(isinstance(claims, list) and claims, "zero claims checked")
    seen = set()
    required = {"id", "section", "title", "value", "unit", "scope", "status",
                "evidence_kind", "source_ref", "public", "sanitization_review", "owner",
                "verification", "note", "semantics"}
    for c in claims:
        require(isinstance(c, dict) and required <= set(c)
                and set(c) <= required | {"on_miss", "failure_signal"}, "invalid claim fields")
        cid = c["id"]
        require(isinstance(cid, str) and re.fullmatch(
            r"(?:[A-Z][0-9]{2}|[a-z]+(?:-[a-z]+)+)", cid), "invalid claim ID")
        require(cid not in seen, f"duplicate claim: {cid}")
        seen.add(cid)
        for key in ("title", "unit", "scope", "status", "evidence_kind", "owner", "note", "sanitization_review"):
            require(nonempty(c[key]), f"{cid}: missing {key}")
        require(c["section"] in SECTIONS, f"{cid}: invalid section")
        require(c["scope"] in SCOPES, f"{cid}: unscoped claim")
        require(c["status"] in STATUSES, f"{cid}: invalid status")
        require(c["evidence_kind"] in KINDS, f"{cid}: invalid evidence kind")
        require(c["semantics"] in SEMANTICS, f"{cid}: invalid semantics")
        require(c["public"] is True, f"{cid}: non-public claim")
        require(c["value"] is not None or c["status"] == "unverified", f"{cid}: unset target or measurement")
        value = c["value"]
        require(value is None or type(value) in (str, int, float, list), f"{cid}: invalid value")
        if isinstance(value, list):
            require(value and all(type(v) in (str, int, float) for v in value), f"{cid}: invalid value list")
        if isinstance(value, str):
            require(nonempty(value), f"{cid}: empty value")
        require(isinstance(c["source_ref"], list) and c["source_ref"], f"{cid}: no source")
        for ref in c["source_ref"]:
            public_ref(ref, root)
        verification = c["verification"]
        require(isinstance(verification, dict) and set(verification) == {"scenario", "receipt"},
                f"{cid}: invalid verification")
        require(nonempty(verification["scenario"]), f"{cid}: missing acceptance scenario")
        receipt = verification["receipt"]
        if c["status"] == "measured":
            require(isinstance(receipt, dict) and set(receipt) == {
                "ref", "source_sha", "date", "environment", "denominator", "result"},
                f"{cid}: measured claim needs an executed receipt")
            public_ref(receipt["ref"], root)
            require(bool(re.fullmatch(r"[0-9a-f]{40}", receipt["source_sha"])), f"{cid}: receipt source SHA")
            date.fromisoformat(receipt["date"])
            require(type(receipt["denominator"]) is int and receipt["denominator"] > 0,
                    f"{cid}: empty measurement denominator")
            require(nonempty(receipt["environment"]) and nonempty(receipt["result"]),
                    f"{cid}: receipt environment/result missing")
            require("design-basis.md" not in receipt["ref"], f"{cid}: a decision is not an executed receipt")
        else:
            require(receipt is None, f"{cid}: reconcile receipt and evidence status")
        if c["section"] == "service":
            require(nonempty(c.get("on_miss")) and nonempty(c.get("failure_signal")),
                    f"{cid}: service claim lacks miss behavior or visible signal")
        # P04 is the separately approved owner quota, not a viewer/occupancy alert.
        if cid in {"P01", "P02", "P03", "D13", "D14"}:
            require(c["semantics"] == "advisory-alert", f"{cid}: capacity must be advisory")
    require(SECTIONS <= {c["section"] for c in claims}, "missing claim section")
    require(SUMMARY <= seen, "missing summary claim")
    return data


def cell(value):
    return str(value).replace("|", "&#124;").replace("\n", " ")


def expectation(c):
    v = c["value"]
    if v is None:
        return f"Pending ({c['unit']})"
    if isinstance(v, list):
        v = ", ".join(str(x) for x in v)
    if isinstance(c["value"], str):
        return v
    if c["unit"] == "seconds maximum target":
        return f"≤ {v} seconds (target)"
    if c["unit"] == "minutes exceeded":
        return f"> {v} minutes"
    return f"{v} {c['unit']}"


def table(claims):
    lines = ["| Claim | Expectation | Scope / status | Source and boundary |",
             "|---|---|---|---|"]
    for c in claims:
        label = f"{c['id']} — {c['title']}" if re.fullmatch(r"[A-Z][0-9]{2}", c["id"]) else c["title"]
        refs = " ".join(f"[source {i}]({ref})" for i, ref in enumerate(c["source_ref"], 1))
        receipt = c["verification"]["receipt"]
        if receipt:
            refs += f" [receipt]({receipt['ref']})"
        lines.append("| " + " | ".join(cell(x) for x in (
            f"**{label}**", expectation(c),
            f"{c['scope']} / **{c['status']}** ({c['evidence_kind']})",
            f"{refs} {c['note']}")) + " |")
    return "\n".join(lines)


def render(data, block):
    claims = data["claims"]
    if block == "metadata":
        m = data["specification"]
        state = (f"Release tag: `{m['release_tag']}`." if m["release_tag"] else
                 f"**{m['product_version']} beta release specifications; release tag pending.**")
        return (f"Applies to: **{m['applies_to']}** · Product: **{m['product_version']}** · "
                f"Specification revision: **{m['revision']}**.\n\n{state} "
                f"Source baseline: [{m['source_sha'][:8]}](https://github.com/cybrrd/brrdfeeder/tree/{m['source_sha']}). "
                "Future-v5 rows are separate design targets, not beta features.")
    if block == "summary":
        return table([c for c in claims if c["id"] in SUMMARY])
    if block == "service-methods":
        lines = ["| Claim | Measurement scenario | On a miss | Failure signal |", "|---|---|---|---|"]
        for c in claims:
            if c["section"] == "service":
                lines.append("| " + " | ".join(cell(x) for x in (
                    c["id"], c["verification"]["scenario"], c["on_miss"], c["failure_signal"])) + " |")
        return "\n".join(lines)
    selected = [c for c in claims if c["section"] == block]
    result = table(selected)
    if block == "placeholders":
        result += "\n\n| Claim | Owner / acceptance still needed |\n|---|---|\n"
        result += "\n".join(f"| {c['id']} | {cell(c['owner'])}: {cell(c['verification']['scenario'])} |"
                            for c in selected)
    return result


def update_pages(data, root, check):
    stale = []
    for name, blocks in PAGES.items():
        path = root / "docs/specs" / name
        original = path.read_text()
        expected = ["metadata", *blocks]
        starts = re.findall(r"<!-- BEGIN GENERATED: ([\w-]+) -->", original)
        ends = re.findall(r"<!-- END GENERATED: ([\w-]+) -->", original)
        require(starts == expected and ends == expected, f"{name}: missing/duplicate/unknown generated blocks")
        text = original
        for block in expected:
            begin, end = f"<!-- BEGIN GENERATED: {block} -->", f"<!-- END GENERATED: {block} -->"
            pattern = re.escape(begin) + r".*?" + re.escape(end)
            replacement = begin + "\n" + render(data, block) + "\n" + end
            text = re.sub(pattern, lambda _: replacement, text, flags=re.S)
        if text != original:
            stale.append(name)
            if not check:
                path.write_text(text)
    require(not check or not stale, "stale generated content: " + ", ".join(stale))
    return stale


def check_links(root):
    """Check local file/heading links in the authored suite, without network calls."""
    for path in (root / "docs/specs").rglob("*.md"):
        for ref in re.findall(r"\]\(([^)]+)\)", path.read_text()):
            url = urlsplit(ref)
            if url.scheme or url.netloc:
                continue
            dest = (path.parent / url.path).resolve() if url.path else path
            require(dest.is_file(), f"{path.name}: broken local link: {ref}")
            if url.fragment and dest.suffix == ".md":
                anchors = {slug(line.lstrip("# ").strip()) for line in dest.read_text().splitlines()
                           if line.startswith("#")}
                require(url.fragment in anchors, f"{path.name}: broken heading link: {ref}")


def run(root=ROOT, check=False):
    data = validate(load(root / "docs/specs/claims.yaml"), root)
    update_pages(data, root, check)
    check_links(root)
    counts = Counter(c["status"] for c in data["claims"])
    print(f"specs: {len(data['claims'])} claims; " + ", ".join(f"{s}={counts[s]}" for s in sorted(STATUSES))
          + ("; generated content current" if check else "; tables generated"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail instead of writing stale views")
    args = parser.parse_args()
    try:
        run(check=args.check)
    except (ValueError, OSError, TypeError, KeyError) as exc:
        print(f"specs: FAIL: {exc}", file=sys.stderr)
        sys.exit(1)
