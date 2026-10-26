// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! One JSON request on stdin, one JSON response. No network or radio access.
use cybrrd_rid_v5::{auth, capture, codec, decode_message, hex, schema, unhex};
use serde_json::{json, Value};
use std::io::{self, Read, Write};

fn run(request: &Value) -> Result<Value, String> {
    match request["mode"].as_str().ok_or("mode required")? {
        "message" => Ok(decode_message(
            &unhex(request["hex"].as_str().ok_or("hex required")?)?,
            0,
        )),
        "unit" => Ok(cybrrd_rid_v5::decoder::decode_unit(
            &unhex(request["hex"].as_str().ok_or("hex required")?)?,
            0,
            0,
            None,
            request["pack"].as_bool().ok_or("pack required")?,
        )),
        "capture" => Ok(serde_json::to_value(capture::decode_capture(
            serde_json::from_value(request["capture"].clone()).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?),
        "fixture" => Ok(
            serde_json::to_value(capture::decode_capture(capture::fixture_input(
                &request["record"],
            )?))
            .map_err(|e| e.to_string())?,
        ),
        "assemble" => {
            let pages =
                serde_json::from_value(request["pages"].clone()).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(auth::assemble(
                pages,
                request["assembly_id"].as_str().ok_or("assembly_id")?,
                request["revision"].as_u64().ok_or("revision")?,
                request["assembler_version"].as_str().unwrap_or("v5-p0-0"),
            ))
            .map_err(|e| e.to_string())?)
        }
        "parcel" => Ok(codec::inspect_parcel(
            &unhex(request["hex"].as_str().ok_or("hex")?)?,
            request["media_type"].as_str().ok_or("media_type")?,
        )),
        "codec" => {
            let shape = match request["shape"].as_str() {
                Some("message") => schema::message(),
                Some("assembly") => schema::assembly(),
                Some("observation") => schema::observation(),
                _ => &Value::Null,
            };
            let v = &request["logical"];
            let cbor = codec::canonical_cbor(v, shape)?;
            let json = codec::canonical_json(v)?;
            let decoded = codec::decode_cbor(&cbor, shape)?;
            Ok(json!({"json_hex":hex(&json),"cbor_hex":hex(&cbor),"decoded":decoded}))
        }
        "decode_cbor" => {
            let shape = match request["shape"].as_str() {
                Some("message") => schema::message(),
                Some("assembly") => schema::assembly(),
                Some("observation") => schema::observation(),
                _ => &Value::Null,
            };
            codec::decode_cbor(&unhex(request["hex"].as_str().ok_or("hex")?)?, shape)
        }
        "verify" => {
            schema::verify_expected(&request["expected"], &request["actual"])?;
            Ok(json!({"verified":true}))
        }
        _ => Err("unsupported offline mode".into()),
    }
}
fn main() {
    let mut bytes = Vec::new();
    // A parcel budget does not authorize input loss. Read all bytes for the
    // quarantine response; bounded codec/tree work is enforced separately.
    if let Err(e) = io::stdin().read_to_end(&mut bytes) {
        eprintln!("stdin: {e}");
        std::process::exit(2);
    }
    let result = codec::decode_json(&bytes).and_then(|v| run(&v));
    let failed = result.is_err();
    let out = match result {
        Ok(v) => v,
        Err(e) => json!({"disposition":"quarantine","reason":e,"exact_input_hex":hex(&bytes)}),
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &out).is_err() || stdout.write_all(b"\n").is_err() {
        std::process::exit(2);
    }
    if failed {
        std::process::exit(1);
    }
}
