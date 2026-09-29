#!/usr/bin/env python3
"""Generate reproducible fuzz seeds, never edit source vectors.

The session JSONL is wholly invented normalized telemetry, NOT captured RF.
Its seeds cover identity, location, a shared operator station and operator ID 0.
"""
import contextlib
import hashlib
import importlib.util
import io
import json
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parents[1]
REPO = ROOT.parents[1]
OUT = ROOT / "fuzz/corpus"
counts = {}
def seed(target, data):
    folder = OUT / target
    folder.mkdir(parents=True, exist_ok=True)
    name = hashlib.sha256(data).hexdigest()
    path = folder / name
    if not path.exists():
        path.write_bytes(data)
        counts[target] = counts.get(target, 0) + 1

def pack_seeds(pack, mac=b"\x01" * 6):
    seed("pack", pack)
    if len(pack) <= 245:
        seed("ble", bytes([5 + len(pack), 0x16, 0xfa, 0xff, 0x0d, 0]) + pack)
    for off in range(3, len(pack), 25):
        single = pack[off:off+25]
        if len(single) == 25:
            seed("pack", single)
            seed("ble", bytes([30, 0x16, 0xfa, 0xff, 0x0d, 0]) + single)
    if len(pack) <= 251:
        header = bytearray(36); header[0] = 0x80; header[10:16] = mac
        seed("wifi", header + bytes([0xdd, 4 + len(pack), 0xfa, 0x0b, 0xbc, 0x0d]) + pack)

vectors = REPO / "Standards/F3411/vectors"
for path in sorted(vectors.glob("*.pcap")):
    raw = path.read_bytes()
    if raw[:4] != bytes.fromhex("d4c3b2a1"):
        raise ValueError("unexpected pcap byte order")
    link, = struct.unpack_from("<I", raw, 20)
    off = 24
    while off + 16 <= len(raw):
        length, = struct.unpack_from("<I", raw, off+8)
        packet = raw[off+16:off+16+length]; off += 16+length
        if link == 127:
            rtlen, = struct.unpack_from("<H", packet, 2)
            packet = packet[rtlen:]
        elif link != 105:
            raise ValueError(f"unexpected link type {link}")
        seed("wifi", packet)
        ie = 36
        while ie+2 <= len(packet):
            end = ie+2+packet[ie+1]
            if end > len(packet): break
            if packet[ie] == 0xdd and packet[ie+2:ie+6] == bytes.fromhex("fa0bbc0d"):
                pack_seeds(packet[ie+6:end])
            ie = end

spec = importlib.util.spec_from_file_location("ble_vectors", ROOT / "tools/ble_vector_extract.py")
extractor = importlib.util.module_from_spec(spec); spec.loader.exec_module(extractor)
capture = io.StringIO()
with contextlib.redirect_stdout(capture):
    extractor.extract(vectors / "odid_bt5_lr_sample.pcapng")
for line in capture.getvalue().splitlines():
    if line.startswith("first_ad_for_header_"):
        ad = bytes.fromhex(line.split("=", 1)[1]); seed("ble", ad); pack_seeds(ad[6:])

session = ROOT / "cybrrd-rid-protocol/tests/fixtures/synthetic-ble-session.jsonl"
for line in session.read_text().splitlines():
    row = json.loads(line)["data"]
    pos = row["pos"]
    basic = bytearray(25); basic[0] = 0x02; basic[1] = 0x10
    serial = row["drone_id"].encode()[:20]; basic[2:2+len(serial)] = serial
    location = bytearray(25); location[0] = 0x12
    struct.pack_into("<ii", location, 5, round(pos["lat"] * 1e7), round(pos["lon"] * 1e7))
    struct.pack_into("<H", location, 15, round((pos["alt_m"] + 1000) * 2))
    station = bytearray(25); station[0] = 0x42
    operator = row['operator_pos']
    struct.pack_into('<ii', station, 2, round(operator['lat']*1e7), round(operator['lon']*1e7))
    struct.pack_into('<H', station, 18, round((operator['alt_m']+1000)*2))
    operator_id = bytearray(25); operator_id[0] = 0x52
    identity = row['operator_id'].encode(); operator_id[2:2+len(identity)] = identity
    messages = ([basic] if row['drone_id'] != 'UNKNOWN' else []) + [location,station,operator_id]
    pack_seeds(bytes([0xf2,25,len(messages)]) + b''.join(messages),bytes.fromhex(row['mac_address'].replace(':','')))
print(json.dumps({"new_seeds": counts, "session": "wholly synthetic fixture; no captured identities or coordinates"}, sort_keys=True))
