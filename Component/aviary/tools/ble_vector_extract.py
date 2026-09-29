#!/usr/bin/env python3
"""Offline vector extraction ONLY; not a product-path ODID decoder.

Reads the supplied pcapng's Nordic v3 capture wrapper (LINKTYPE 272),
not LINKTYPE_BLUETOOTH_LE_LL_WITH_PHDR (256). Layout reference:
https://github.com/wireshark/wireshark/blob/master/epan/dissectors/packet-nordic_ble.c
No radio, network, file writes, or ODID sub-message decoding.
"""
import collections
import pathlib
import struct
import sys


def extract(path):
    data = pathlib.Path(path).read_bytes()
    counts = collections.Counter()
    headers = collections.Counter()
    first = {}
    interfaces = []
    offset = 0
    while offset < len(data):
        kind, size = struct.unpack_from("<II", data, offset)
        if size < 12 or size % 4 or offset + size > len(data):
            raise ValueError(f"bad pcapng block at {offset}")
        block = data[offset:offset + size]
        if struct.unpack_from("<I", block, size - 4)[0] != size:
            raise ValueError("pcapng length trailer mismatch")
        if kind == 0x0A0D0D0A:
            if block[8:12] != bytes.fromhex("4d3c2b1a"):
                raise ValueError("only little-endian capture supported")
            interfaces = []
        elif kind == 1:
            interfaces.append(struct.unpack_from("<H", block, 8)[0])
        elif kind == 6:
            counts["epb"] += 1
            interface, _, _, caplen, original = struct.unpack_from("<IIIII", block, 8)
            if interfaces[interface] != 272 or caplen != original or 28 + caplen > size - 4:
                raise ValueError("unexpected or truncated capture")
            p = block[28:28 + caplen]
            if len(p) < 24 or p[3] != 3 or struct.unpack_from("<H", p, 1)[0] + 7 != len(p) or p[7] != 10:
                raise ValueError("unexpected Nordic v3 wrapper")
            if p[6] != 2 or not p[8] & 1:
                counts["not_advertising_or_bad_crc"] += 1
                offset += size
                continue
            phy = (p[8] >> 4) & 7
            ll = p[17:]
            if ll[:4] != bytes.fromhex("d6be898e"):
                raise ValueError("unexpected advertising access address")
            h = 4 + (phy == 2)  # Coded PHY adds the coding indicator.
            if p[9] >= 37 or (p[8] >> 1) & 3 or ll[h] & 15 != 7:
                counts["not_aux_adv_ind"] += 1
                offset += size
                continue
            counts["aux_adv_ind"] += 1
            body = ll[h + 2:h + 2 + ll[h + 1]]
            extlen = body[0] & 63
            if len(body) != ll[h + 1] or extlen + 1 > len(body):
                raise ValueError("truncated extended advertising PDU")
            ad = body[1 + extlen:]
            cursor = 0
            while cursor < len(ad) and ad[cursor]:
                end = cursor + 1 + ad[cursor]
                if end > len(ad):
                    raise ValueError("truncated AD structure")
                service = ad[cursor + 1:end]
                if service[:4] == bytes.fromhex("16faff0d"):
                    counts["signature_matches"] += 1
                    payload = service[5:]
                    key = payload[:3].hex()
                    headers[key] += 1
                    first.setdefault(key, ad[cursor:end].hex())
                cursor = end
        offset += size
    print(f"bytes={len(data)} interfaces={interfaces}")
    print(f"counts={dict(counts)}")
    print(f"pack_headers={dict(headers)}")
    for key, ad_hex in first.items():
        print(f"first_ad_for_header_{key}={ad_hex}")


if __name__ == "__main__":
    extract(sys.argv[1])
