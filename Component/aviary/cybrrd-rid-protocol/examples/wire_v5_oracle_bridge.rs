// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Offline test bridge. No network or filesystem writes.
#[cfg(feature = "wire-v5")]
fn main() {
    use cybrrd_rid_protocol::v5::{self, codec, model, schema};
    use serde_json::{json, Value};
    use std::io::{self, BufRead};
    for line in io::stdin().lock().lines() {
        let result = (|| -> v5::Result<Value> {
            let line = line.map_err(|e| v5::Error(e.to_string()))?;
            let command = codec::parse_json(line.as_bytes())?;
            let record = &command["record"];
            let message = command["message"] == true;
            let s = if message {
                schema::message_contract()?
            } else {
                schema::contract()?
            };
            match command["op"].as_str() {
                Some("codec") => Ok(
                    json!({"json_hex":v5::hex(&codec::canonical_json(record)?),"cbor_hex":v5::hex(&codec::canonical_cbor(record,s)?)}),
                ),
                Some("validate") => {
                    if message {
                        model::validate_message(record)?;
                    } else {
                        model::validate(record)?;
                    }
                    Ok(json!(true))
                }
                Some("decode_cbor") => {
                    let b = v5::unhex(
                        command["hex"]
                            .as_str()
                            .ok_or_else(|| v5::Error("hex required".into()))?,
                    )?;
                    Ok(codec::parse_cbor(&b, s)?)
                }
                Some("decode_message") => {
                    let b = v5::unhex(
                        command["hex"]
                            .as_str()
                            .ok_or_else(|| v5::Error("hex required".into()))?,
                    )?;
                    v5::decode_message(&b, 0)
                }
                _ => Err(v5::Error("unknown test command".into())),
            }
        })();
        let out = match result {
            Ok(v) => json!({"ok":v}),
            Err(e) => json!({"error":e.to_string()}),
        };
        println!("{out}");
    }
}
#[cfg(not(feature = "wire-v5"))]
fn main() {
    eprintln!("requires wire-v5 feature");
    std::process::exit(2);
}
