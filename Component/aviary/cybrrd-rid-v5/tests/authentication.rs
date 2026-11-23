// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use cybrrd_rid_v5::{
    auth::{assemble, PageEvidence},
    capture::ObservationId,
};
use serde_json::json;
fn page(typ: u8, number: u8) -> PageEvidence {
    let mut b = vec![0; 25];
    b[0] = 0x22;
    b[1] = typ * 16 + number;
    PageEvidence {
        observation_id: ObservationId {
            node_id: "synthetic-node".into(),
            boot_id: "synthetic-boot".into(),
            capture_sequence: "1".into(),
        },
        unit_index: 0,
        message_index: number as usize,
        bytes: b,
        source_mac: Some("020000000001".into()),
        transport: "wifi_beacon".into(),
        counter: Some(7),
        association: Some("synthetic-explicit-generation".into()),
    }
}
#[test]
fn sixteen_assembly_boundary_cases() {
    for case in 0..16 {
        let mut p0 = page(1, 0);
        p0.bytes[2] = 1;
        p0.bytes[3] = 1;
        p0.bytes[8] = 0x42;
        let p1 = page(1, 1);
        let mut pages = vec![p0.clone(), p1.clone()];
        let expected = match case {
            0 => {
                pages.reverse();
                "complete"
            }
            1 => {
                pages.pop();
                "incomplete"
            }
            2 => {
                pages.push(p1.clone());
                "complete"
            }
            3 => {
                let mut c = p1.clone();
                c.bytes[2] = 1;
                pages.push(c);
                "conflicting"
            }
            4 => {
                pages[0].counter = Some(255);
                pages[1].counter = Some(0);
                "ambiguous"
            }
            5 => {
                pages[1].observation_id.capture_sequence = "2".into();
                pages[1].association = Some("different-generation".into());
                "ambiguous"
            }
            6 => {
                pages[0].bytes[3] = 0;
                pages[0].bytes[8] = 0;
                "complete"
            }
            7 => {
                pages = (0..16).map(|n| page(1, n)).collect();
                pages[0].bytes[2] = 15;
                pages[0].bytes[3] = 255;
                "complete"
            }
            8 => {
                pages[0].bytes[9] = 0;
                "complete"
            }
            9 => {
                pages[0].bytes[9] = 2;
                pages[0].bytes[10] = 1;
                pages[0].bytes[11] = 2;
                "complete"
            }
            10 => {
                pages[0].bytes[9] = 255;
                "incomplete"
            }
            11 => "complete",
            12 => {
                pages[1].bytes[24] = 1;
                "complete"
            }
            13 => {
                pages[0].bytes[3] = 255;
                "incomplete"
            }
            14 => {
                pages[0].bytes[2] = 0;
                "ambiguous"
            }
            _ => {
                pages[1].observation_id.boot_id = "another-boot".into();
                "ambiguous"
            }
        };
        let original = serde_json::to_value(&pages).unwrap();
        let got = assemble(pages, "synthetic-boundary", 1, "v5-p0-0");
        assert_eq!(got.derived["state"], expected, "case {case}");
        assert_eq!(serde_json::to_value(got.pages).unwrap(), original);
        assert!(got.derived.get("verified").is_none());
        if case == 6 {
            assert_eq!(got.derived["payload_hex"], "");
        }
        if case == 9 {
            assert_eq!(got.derived["additional_data_hex"], "0102");
        }
        if case == 12 {
            assert!(got.derived["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "nonzero_auth_padding"));
        }
        if case == 3 {
            assert_eq!(got.derived["page_refs"].as_array().unwrap().len(), 3);
        }
    }
}
#[test]
fn type3_transport_and_type4_null_rules_five_cases() {
    for case in 0..3 {
        let mut pages = vec![page(3, 0), page(3, 1)];
        pages[0].bytes[2] = 1;
        if case == 1 {
            for p in &mut pages {
                p.transport = "bt4_legacy".into();
            }
        }
        if case == 2 {
            pages[1].observation_id.capture_sequence = "2".into();
        }
        let got = assemble(pages, "synthetic-type3", 1, "v5-p0-0");
        let issue = got.derived["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "auth_type3_transport_constraint");
        assert_eq!(issue, case != 0);
    }
    for nonzero in [false, true] {
        let mut p = page(4, 0);
        p.bytes[8] = u8::from(nonzero);
        let got = assemble(vec![p], "synthetic-type4", 1, "v5-p0-0");
        assert_eq!(
            got.derived["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "network_auth_nonzero_data"),
            nonzero
        );
        assert_ne!(got.derived["state"], json!("verified"));
    }
}
