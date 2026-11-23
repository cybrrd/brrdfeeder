# SPDX-License-Identifier: AGPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Macawi LLC
"""Independent Python/schema/codec checks of actual Rust decoder results.

Requires jsonschema; never imports a production decoder or generates expected
message values from it. The checked-in design fixtures are the expected oracle.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import jsonschema
from semantic_oracle import run as semantic_run

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "contract"


def load(name):
    return json.loads((CONTRACT / name).read_text())


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def reference_cbor(value, shape, dictionary):
    """Independent encoder using Python integers/struct and the frozen schema."""
    def head(major, value):
        if value < 24:
            return bytes([major * 32 + value])
        for width, fmt, limit in [(24, "B", 256), (25, "H", 65536), (26, "I", 2**32), (27, "Q", 2**64)]:
            if value < limit:
                return bytes([major * 32 + width]) + struct.pack(">" + fmt, value)
        raise AssertionError("outside CBOR integer domain")

    def encode(v, s):
        if "anyOf" in s:
            s = next(x for x in s["anyOf"] if x.get("type") != "null")
        if v is None:
            return b"\xf6"
        if isinstance(v, bool):
            return b"\xf5" if v else b"\xf4"
        if isinstance(v, int):
            assert abs(v) <= 2**53 - 1
            return head(0, v) if v >= 0 else head(1, -v - 1)
        if isinstance(v, str):
            if s.get("x-parquet") == "BYTE_ARRAY(hex decoded)":
                b = bytes.fromhex(v)
                assert b.hex() == v
                return head(6, 60000) + head(2, len(b)) + b
            b = v.encode()
            return head(3, len(b)) + b
        if isinstance(v, list):
            return head(4, len(v)) + b"".join(encode(x, s.get("items", {})) for x in v)
        if isinstance(v, dict):
            entries = [(encode(dictionary.get(k, k), {}), encode(x, s.get("properties", {}).get(k, {}))) for k, x in v.items()]
            entries.sort(key=lambda kv: (len(kv[0]), kv[0]))
            return head(5, len(entries)) + b"".join(k + x for k, x in entries)
        raise AssertionError("outside canonical domain")
    return encode(value, shape)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/v5-test-ingress")
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()

    def ingress(request):
        result = subprocess.run([str(args.binary)], input=canonical(request), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
        return json.loads(result.stdout)

    schema = load("observation.schema.json")
    assembly_schema = load("authentication-assembly.schema.json")
    association_schema = load("capture-association.schema.json")
    message_schema = schema["properties"]["rid_units"]["items"]["properties"]["messages"]["items"]
    dictionary = load("cbor-keys.json")["keys"]
    for shape in [schema, message_schema, assembly_schema]:
        jsonschema.Draft202012Validator.check_schema(shape)
    message_validator = jsonschema.Draft202012Validator(message_schema)
    observation_validator = jsonschema.Draft202012Validator(schema)
    assembly_validator = jsonschema.Draft202012Validator(assembly_schema)
    messages = load("golden-messages.json")["vectors"]
    fixtures = load("golden-observations.json")["fixtures"]
    for vector in messages:
        actual = ingress({"mode": "message", "hex": vector["raw_message_hex"]})
        assert actual == vector["expected"], vector["name"]
        message_validator.validate(actual)
        vector["expected"] = actual
    for fixture in fixtures:
        output = ingress({"mode": "fixture", "record": fixture["record"]})
        actual = output["observation"]
        assert actual == fixture["record"], fixture["name"]
        assert output["raw_capture"]["bytes"] == actual["captures"][0]["raw_hex"]
        observation_validator.validate(actual)
        fixture["record"] = actual
    assembly = load("golden-authentication-assembly.json")
    page_vectors = {v["name"]: v for v in messages}
    pages = []
    for i, ref in enumerate(assembly["page_refs"]):
        pages.append({"observation_id": ref["observation_id"], "unit_index": ref["unit_index"], "message_index": ref["message_index"],
                      "bytes": page_vectors[f"auth_page_{i}"]["raw_message_hex"], "source_mac": "020000000001", "transport": "wifi_beacon", "counter": 7,
                      "association": "synthetic-explicit-identity-clock-generation"})
    actual = ingress({"mode": "assemble", "pages": pages, "assembly_id": assembly["assembly_id"], "revision": assembly["revision"], "assembler_version": assembly["assembler_version"]})
    assert actual["derived"] == assembly
    assert actual["pages"] == pages
    assembly_validator.validate(actual["derived"])

    vectors = [(m["name"], m["expected"], message_schema, "message") for m in messages]
    vectors += [(f["name"], f["record"], schema, "observation") for f in fixtures]
    vectors.append(("assembly", actual["derived"], assembly_schema, "assembly"))
    codec_receipts = []
    for name, value, shape, shape_name in vectors:
        result = ingress({"mode": "codec", "shape": shape_name, "logical": value})
        expected_cbor = reference_cbor(value, shape, dictionary)
        assert result["decoded"] == value
        assert result["json_hex"] == canonical(value).hex(), name
        assert result["cbor_hex"] == expected_cbor.hex(), name
        codec_receipts.append({"name": name, "json_sha256": hashlib.sha256(canonical(value)).hexdigest(), "cbor_sha256": hashlib.sha256(expected_cbor).hexdigest()})

    columns = list(csv.DictReader((CONTRACT / "parquet-columns.csv").open()))
    semantics = semantic_run(schema, messages, fixtures, columns, load("golden-display.json")["cases"], association_schema, load("golden-capture-association.json"))
    required = load("semantic-negative-cases.json")["cases"]
    assert semantics["negative_cases"] == required
    receipt = {"status": "passed", "message_vectors": len(messages), "observation_vectors": len(fixtures), "authentication_assemblies": 1,
               "independent_codec_cases": len(codec_receipts), "codec_receipts": codec_receipts, "semantic": semantics,
               "boundary": "offline decoder, schema and independent codec; display/lake checks are design contract controls, not application tests"}
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"status": "passed", "messages": len(messages), "observations": len(fixtures), "assemblies": 1,
                      "independent_codec_cases": len(codec_receipts), "semantic_negative_controls": len(required)}))


if __name__ == "__main__":
    main()
