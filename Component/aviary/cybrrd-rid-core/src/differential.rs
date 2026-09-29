// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! D18 differential oracle: frozen from f6ac1eca3e07eec9454bd2348720d1a91a409fda.
//! Only the function name differs. Shared helpers are unchanged by D18.
//! D27 Wave E adds metadata/publication semantics: compare the complete legacy
//! projection below; absolute position_v4 tests cover the new contract.
use super::*;
use std::path::Path;

// Preserve the pre-change implementation verbatim, including its two lints.
#[rustfmt::skip]
#[allow(clippy::chunks_exact_to_as_chunks, clippy::collapsible_match)]
fn decode_submessages_reference(pack_data: &[u8]) -> RidPack {
    let mut out = RidPack::default();

    for chunk in pack_data.chunks_exact(SUB_MESSAGE_LEN) {
        // chunk is exactly 25 bytes — every chunk[i] for i < 25 is safe.
        match (chunk[0] & 0xF0) >> 4 {
            MSG_TYPE_BASIC_ID => {
                let id_type = IdType::from_nibble((chunk[1] & 0xF0) >> 4);
                if let Some(id) = bounded_id(&chunk[2..22]) {
                    match id_type {
                        IdType::SerialNumber => {
                            if out.hardware_serial.is_none() {
                                out.hardware_serial = Some(id);
                            }
                        }
                        IdType::CaaRegistration => {
                            if out.caa_registration.is_none() {
                                out.caa_registration = Some(id);
                            }
                        }
                        // IDType 0 (None) / UTM / session / reserved: many
                        // consumer drones don't set a meaningful IDType. Treat
                        // an unclassified-but-present ID as the hardware serial
                        // only if we have no typed ID yet — back-compatible
                        // with the prior single-field behaviour.
                        _ => {
                            if out.hardware_serial.is_none() && out.caa_registration.is_none() {
                                out.hardware_serial = Some(id);
                            }
                        }
                    }
                }
            }
            MSG_TYPE_LOCATION => {
                out.drone_pos = decode_location(chunk);
            }
            MSG_TYPE_SYSTEM => {
                // OperatorLatitude/Longitude i32 LE @1e-7 at bytes 2..10,
                // OperatorAltitudeGeo u16 LE (+1000m, 0.5m res) at bytes 18..20.
                let lat = i32::from_le_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);
                let lon = i32::from_le_bytes([chunk[6], chunk[7], chunk[8], chunk[9]]);
                let alt_raw = u16::from_le_bytes([chunk[18], chunk[19]]);
                out.operator_pos = validated_point(lat, lon, alt_raw);
            }
            MSG_TYPE_OPERATOR_ID => {
                // byte 1 = OperatorIdType; bytes 2..22 = OperatorId (20B).
                if out.operator_id.is_none() {
                    out.operator_id = bounded_id(&chunk[2..22]);
                }
            }
            MSG_TYPE_SELF_ID => {
                // byte 1 = DescType; bytes 2..25 = Description (23B).
                if out.self_id.is_none() {
                    if let Some(description) = bounded_desc(&chunk[2..25]) {
                        out.self_id = Some(SelfId {
                            desc_type: chunk[1],
                            description,
                        });
                    }
                }
            }
            MSG_TYPE_AUTH => {
                // byte 1 = AuthType (high nibble) | DataPage (low nibble).
                // First seen wins — don't let a continuation page clobber page 0.
                if out.auth.is_none() {
                    let auth_type = (chunk[1] & 0xF0) >> 4;
                    let data_page = chunk[1] & 0x0F;
                    if data_page == 0 {
                        let last_page_index = chunk[2];
                        let total_length = chunk[3];
                        let astm_ts =
                            u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                        out.auth = Some(AuthMeta {
                            auth_type,
                            timestamp_utc: Some(ASTM_EPOCH_UNIX + astm_ts as u64),
                            total_length: Some(total_length),
                            last_page_index: Some(last_page_index),
                        });
                    } else {
                        out.auth = Some(AuthMeta {
                            auth_type,
                            timestamp_utc: None,
                            total_length: None,
                            last_page_index: None,
                        });
                    }
                }
            }
            _ => {} // Types 6/7 reserved; ignored.
        }
    }

    out
}

// Unchanged public-envelope rules, selecting the frozen decoder at the leaf.
fn pack_reference(payload: &[u8]) -> Result<RidPack, ParseError> {
    let p = if !payload.is_empty() && (payload[0] >> 4) != 0xF {
        &payload[1..]
    } else {
        payload
    };
    if p.len() < 3 {
        return Err(ParseError::TooShort);
    }
    if (p[0] >> 4) != 0xF {
        return Err(ParseError::NotAMessagePack);
    }
    if p[1] as usize != SUB_MESSAGE_LEN {
        return Err(ParseError::BadMessageSize);
    }
    let claimed_len = 3 + p[2] as usize * SUB_MESSAGE_LEN;
    let data = if p.len() >= claimed_len {
        &p[3..claimed_len]
    } else {
        let available = p.len().saturating_sub(3) / SUB_MESSAGE_LEN;
        &p[3..3 + available * SUB_MESSAGE_LEN]
    };
    let out = decode_submessages_reference(data);
    if out.drone_pos.is_none() {
        Err(ParseError::NoLocation)
    } else {
        Ok(out)
    }
}

fn identical<T: core::fmt::Debug + PartialEq>(label: &str, input: &[u8], old: T, new: T) {
    // Compare the WHOLE structure, then its complete Debug byte representation.
    // No hand-picked fields, lossy JSON float conversion, or pointer/padding bytes.
    assert_eq!(old, new, "D18 DIFFERENCE: {label}, input={input:02x?}");
    assert_eq!(
        format!("{old:?}").as_bytes(),
        format!("{new:?}").as_bytes(),
        "D18 BYTE DIFFERENCE: {label}, input={input:02x?}"
    );
}

#[derive(Default)]
struct Counts {
    inputs: usize,
    comparisons: usize,
}

// The frozen reference body above is unchanged. Explicit compatibility
// projection removes only the three new D27 metadata fields. All legacy data
// (including decoded positions, identities, auth and strings) is compared.
fn legacy_fields(mut pack: RidPack) -> RidPack {
    pack.operational_status = None;
    pack.position_unknown_reason = None;
    pack.operator_position_unknown_reason = None;
    pack
}

// D18's public pack contract rejected position-less results. Reapply that
// contract ONLY in the oracle projection, never in production. Wave E tests
// independently prove that real publication now retains those observations.
fn legacy_publication(result: Result<RidPack, ParseError>) -> Result<RidPack, ParseError> {
    result.and_then(|pack| if pack.drone_pos.is_none() {
        Err(ParseError::NoLocation)
    } else {
        Ok(legacy_fields(pack))
    })
}

fn check(label: &str, bytes: &[u8], cases: &mut Counts) {
    identical(
        label,
        bytes,
        decode_submessages_reference(bytes),
        legacy_fields(decode_submessages(bytes)),
    );
    identical(
        label,
        bytes,
        pack_reference(bytes),
        legacy_publication(decode_message_pack(bytes)),
    );
    let single = if bytes.len() == SUB_MESSAGE_LEN {
        Ok(decode_submessages_reference(bytes))
    } else {
        Err(ParseError::BadMessageSize)
    };
    identical(label, bytes, single, decode_single_message(bytes).map(legacy_fields));
    cases.inputs += 1;
    cases.comparisons += 3;

    // Check the complete decoded body even if the public pack API discards it
    // as NoLocation; an AUTH/ID difference must never hide behind that error.
    let p = if !bytes.is_empty() && bytes[0] >> 4 != 0xf {
        &bytes[1..]
    } else {
        bytes
    };
    if p.len() >= 3 && p[0] >> 4 == 0xf && p[1] as usize == SUB_MESSAGE_LEN {
        let count = usize::from(p[2]).min((p.len() - 3) / SUB_MESSAGE_LEN);
        let body = &p[3..3 + count * SUB_MESSAGE_LEN];
        identical(
            label,
            body,
            decode_submessages_reference(body),
            legacy_fields(decode_submessages(body)),
        );
        cases.comparisons += 1;
    }
}

fn framed(body: &[u8], claimed_count: u8) -> Vec<u8> {
    let mut bytes = vec![0xf2, SUB_MESSAGE_LEN as u8, claimed_count];
    bytes.extend_from_slice(body);
    bytes
}

fn corpus(root: &Path, cases: &mut Counts) -> usize {
    // Use the actual repository generator, not a rewritten copy of its seeds.
    // Python is existing repository tooling, not a new Cargo dependency.
    // This writes only the normal ignored fuzz/corpus directory. Fuzz runs in
    // the D18 proof use a separate corpus copy so they cannot mutate these seeds.
    let generated = std::process::Command::new("python3")
        .arg("-B")
        .arg(root.join("fuzz/seed.py"))
        .output()
        .expect("run existing fuzz seed generator (Python 3 required)");
    assert!(
        generated.status.success(),
        "seed.py failed: {}",
        std::string::String::from_utf8_lossy(&generated.stderr)
    );
    let mut total = 0;
    let mut extracted = 0;
    for target in ["pack", "ble", "wifi"] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join("fuzz/corpus").join(target))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "missing {target} corpus");
        println!("D18 corpus {target}: {} seeds", paths.len());
        total += paths.len();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap();
            let label = path.display().to_string();
            check(&label, &bytes, cases);
            // Also decode the real ODID payloads at their transport boundaries,
            // not just the outer frame's bytes at arbitrary 25-byte alignment.
            let mut offset = if target == "wifi" { 36 } else { 0 };
            if target == "pack" {
                continue;
            }
            while offset + 2 <= bytes.len() {
                let (start, end) = if target == "wifi" {
                    (offset + 2, offset + 2 + bytes[offset + 1] as usize)
                } else {
                    (offset + 1, offset + 1 + bytes[offset] as usize)
                };
                if end > bytes.len() || end <= start {
                    break;
                }
                let field = &bytes[start..end];
                let payload = if target == "wifi"
                    && bytes[offset] == 0xdd
                    && field.starts_with(&[0xfa, 0x0b, 0xbc, 0x0d])
                {
                    Some(&field[4..])
                } else if target == "ble"
                    && field.len() >= 5
                    && field.starts_with(&[0x16, 0xfa, 0xff, 0x0d])
                {
                    Some(&field[5..]) // service header + application counter
                } else {
                    None
                };
                if let Some(payload) = payload {
                    check(&label, payload, cases);
                    extracted += 1;
                }
                offset = end;
            }
        }
    }
    assert!(extracted > 0, "transport extraction must exercise payloads");
    println!("D18 extracted transport payloads: {extracted}");
    total
}

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

#[test]
fn frozen_decoder_legacy_fields_are_byte_identical() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut cases = Counts::default();
    let seeds = corpus(root, &mut cases);
    let corpus_cases = cases.inputs;

    // All 16 type nibbles x ALL 16 version/low nibbles (stronger than a
    // representative low nibble), singly and in packs of every length 1..=9.
    for high in 0..16 {
        for low in 0..16 {
            let mut message = [b'A'; SUB_MESSAGE_LEN];
            message[0] = (high << 4) | low;
            message[1] = 0x10;
            check("type-single", &message, &mut cases);
            for count in 1..=9 {
                let body = message.repeat(count);
                check("type-body", &body, &mut cases);
                check("type-pack", &framed(&body, count as u8), &mut cases);
            }
        }
    }
    let type_cases = cases.inputs - corpus_cases;

    for length in [0, 1, 24, 25, 26, 49, 50, 51, 227] {
        let mut body = vec![0x41; length];
        for header in body.iter_mut().step_by(SUB_MESSAGE_LEN) {
            *header = 0x32; // nonempty SELF_ID, plus truncated tails
        }
        check("length-boundary", &body, &mut cases);
        for claimed in [0, 1, 2, 9, 255] {
            let pack = framed(&body, claimed);
            check("boundary-pack", &pack, &mut cases);
            let mut prefixed = vec![0]; // supported DJI nonstandard prefix
            prefixed.extend_from_slice(&pack);
            check("boundary-prefixed-pack", &prefixed, &mut cases);
        }
    }
    let boundary_cases = cases.inputs - corpus_cases - type_cases;

    for auth_type in 0..16 {
        let mut page0 = [0x5a; SUB_MESSAGE_LEN];
        page0[0] = 0x22;
        page0[1] = auth_type << 4;
        let mut second0 = page0;
        second0[2..].fill(0xa5); // distinguish which page won
        check("auth-two-page-zero", &[page0, second0].concat(), &mut cases);
        for page in 1..16 {
            let mut continuation = second0;
            continuation[1] = (auth_type << 4) | page;
            for body in [
                [page0, continuation].concat(),
                [continuation, page0].concat(),
                continuation.to_vec(),
            ] {
                check("auth-order", &body, &mut cases);
                check("auth-order-pack", &framed(&body, 2), &mut cases);
            }
        }
    }
    let auth_cases = cases.inputs - corpus_cases - type_cases - boundary_cases;

    const RANDOM_SEED: u64 = 0xd18a_2026_0918_0001;
    let mut state = RANDOM_SEED;
    for _ in 0..10_000 {
        let length = (next(&mut state) % 4097) as usize;
        let bytes: Vec<_> = (0..length)
            .map(|_| (next(&mut state) >> 56) as u8)
            .collect();
        check("fixed-seed-random", &bytes, &mut cases);
    }
    println!(
        "D18 PASS seeds={seeds} corpus_cases={corpus_cases} type_cases={type_cases} \
         boundary_cases={boundary_cases} auth_cases={auth_cases} random_cases=10000 \
         random_seed={RANDOM_SEED:#x} total_inputs={} legacy_output_comparisons={} mismatches=0",
        cases.inputs, cases.comparisons
    );
}
