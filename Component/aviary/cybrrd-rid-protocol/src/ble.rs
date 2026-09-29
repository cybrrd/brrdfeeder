//! BLE AD de-framing only. All ODID fields are decoded by the shared no_std core.
use crate::astm::telemetry_from_pack;
use crate::models::{RidTransport, TelemetryData};
use cybrrd_rid_core::{decode_message_pack, decode_single_message, RidPack};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlePhy {
    Le1M,
    LeCoded,
}

#[derive(Clone, Copy, Debug)]
pub struct BleMeta {
    pub phy: BlePhy,
    pub extended: bool,
}

impl BleMeta {
    fn transport(self) -> Result<RidTransport, Reject> {
        match (self.extended, self.phy) {
            (false, BlePhy::Le1M) => Ok(RidTransport::Bt4Legacy),
            (true, BlePhy::LeCoded) => Ok(RidTransport::Bt5LongRange),
            _ => Err(Reject::UnsupportedTransport),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Reject {
    TruncatedAd,
    NoSignature,
    BadPayload,
    PackCount,
    CoreDecode,
    NoLocation,
    UnsupportedTransport,
}

static REJECTS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// Counts indexed by `Reject`; a source can publish these in its health surface.
pub fn reject_counts() -> [u64; 7] {
    std::array::from_fn(|i| REJECTS[i].load(Ordering::Relaxed))
}

fn counted<T>(result: Result<T, Reject>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(reason) => {
            REJECTS[reason as usize].fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

fn decode_ad(ad: &[u8], meta: BleMeta) -> Result<(RidPack, u8, RidTransport), Reject> {
    let transport = meta.transport()?;
    let mut offset = 0;
    while offset < ad.len() {
        let len = usize::from(ad[offset]);
        if len == 0 {
            break;
        }
        let end = offset + 1 + len;
        let service = ad.get(offset + 1..end).ok_or(Reject::TruncatedAd)?;
        if service.starts_with(&[0x16, 0xfa, 0xff, 0x0d]) {
            let counter = *service.get(4).ok_or(Reject::BadPayload)?;
            let payload = &service[5..];
            let pack = match transport {
                RidTransport::Bt5LongRange => {
                    // No DJI prefix on BLE: the vector's header starts at byte 0.
                    if payload.len() < 3 || payload[0] >> 4 != 15 || payload[1] != 25 {
                        return Err(Reject::BadPayload);
                    }
                    // BB50120 is a BLE boundary; preserve existing Wi-Fi behavior.
                    if !(1..=9).contains(&payload[2]) {
                        return Err(Reject::PackCount);
                    }
                    let required = 3 + usize::from(payload[2]) * 25;
                    if payload.len() < required {
                        return Err(Reject::BadPayload);
                    }
                    decode_message_pack(payload).map_err(|_| Reject::CoreDecode)?
                }
                RidTransport::Bt4Legacy => {
                    if payload.len() != 25 || payload[0] >> 4 > 5 {
                        return Err(Reject::BadPayload);
                    }
                    decode_single_message(payload).map_err(|_| Reject::CoreDecode)?
                }
                _ => return Err(Reject::UnsupportedTransport),
            };
            return Ok((pack, counter, transport));
        }
        offset = end;
    }
    Err(Reject::NoSignature)
}

/// Stateless entry point. A single Basic ID has no position and emits nothing.
pub fn ingest_ad(ad: &[u8], addr: [u8; 6], rssi_dbm: i32, meta: BleMeta) -> Option<TelemetryData> {
    counted(ingest_ad_result(ad, addr, rssi_dbm, meta))
}

pub fn ingest_ad_result(
    ad: &[u8],
    addr: [u8; 6],
    rssi_dbm: i32,
    meta: BleMeta,
) -> Result<TelemetryData, Reject> {
    let (pack, counter, transport) = decode_ad(ad, meta)?;
    let mut data = telemetry_from_pack(pack, addr, rssi_dbm).ok_or(Reject::NoLocation)?;
    data.transport = Some(transport);
    data.message_counter = Some(counter);
    Ok(data)
}

/// Bounded, receiver-local identity association across both PHYs. Only Location
/// arrivals emit; unidentified locations are dropped, never replayed later.
/// TTL is measured from the last Basic ID, not refreshed by associated locations.
#[derive(Default)]
pub struct BleDecoder {
    identities: HashMap<[u8; 6], (Instant, RidPack)>,
    unassociated_dropped: u64,
}

impl BleDecoder {
    /// Drain into the engine's rate-limited rid_ble_unassociated_dropped audit.
    pub fn take_unassociated_dropped(&mut self) -> u64 {
        std::mem::take(&mut self.unassociated_dropped)
    }

    pub fn ingest(
        &mut self,
        ad: &[u8],
        addr: [u8; 6],
        rssi: i32,
        meta: BleMeta,
    ) -> Option<TelemetryData> {
        self.ingest_at(ad, addr, rssi, meta, Instant::now())
    }

    fn ingest_at(
        &mut self,
        ad: &[u8],
        addr: [u8; 6],
        rssi: i32,
        meta: BleMeta,
        now: Instant,
    ) -> Option<TelemetryData> {
        let (mut pack, counter, transport) = counted(decode_ad(ad, meta))?;
        self.identities.retain(|_, (seen, _)| {
            now.saturating_duration_since(*seen) < Duration::from_secs(60)
        });
        {
            if pack.hardware_serial.is_some() || pack.caa_registration.is_some() {
                if self.identities.len() >= 128 && !self.identities.contains_key(&addr) {
                    if let Some(oldest) = self
                        .identities
                        .iter()
                        .min_by_key(|(_, (seen, _))| *seen)
                        .map(|(addr, _)| *addr)
                    {
                        self.identities.remove(&oldest);
                    }
                }
                // Cache identity types only, never position/operator/auth data.
                let identity = RidPack {
                    hardware_serial: pack.hardware_serial.clone(),
                    caa_registration: pack.caa_registration.clone(),
                    ..RidPack::default()
                };
                self.identities.insert(addr, (now, identity));
            }
            if let Some((_, identity)) = self.identities.get(&addr) {
                pack.hardware_serial = pack
                    .hardware_serial
                    .or_else(|| identity.hardware_serial.clone());
                pack.caa_registration = pack
                    .caa_registration
                    .or_else(|| identity.caa_registration.clone());
            }
        }
        if pack.operational_status.is_some() && pack.hardware_serial.is_none() && pack.caa_registration.is_none() {
            self.unassociated_dropped = self.unassociated_dropped.saturating_add(1);
            return None;
        }
        let mut data = telemetry_from_pack(pack, addr, rssi)?;
        data.transport = Some(transport);
        data.message_counter = Some(counter);
        Some(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BT4: BleMeta = BleMeta {
        phy: BlePhy::Le1M,
        extended: false,
    };
    const BT5: BleMeta = BleMeta {
        phy: BlePhy::LeCoded,
        extended: true,
    };

    fn ad(payload: &[u8], counter: u8) -> Vec<u8> {
        let mut out = vec![(payload.len() + 5) as u8, 0x16, 0xfa, 0xff, 0x0d, counter];
        out.extend_from_slice(payload);
        out
    }

    fn messages() -> ([u8; 25], [u8; 25]) {
        let mut id = [0; 25];
        id[1] = 0x10;
        id[2..6].copy_from_slice(b"TEST");
        let mut loc = [0; 25];
        loc[0] = 0x10;
        loc[5..9].copy_from_slice(&408_817_203i32.to_le_bytes());
        loc[9..13].copy_from_slice(&(-956_901_255i32).to_le_bytes());
        (id, loc)
    }

    fn pack(count: u8) -> Vec<u8> {
        let (id, loc) = messages();
        let mut out = vec![0xf2, 25, count];
        for i in 0..count {
            out.extend_from_slice(if i == 0 { &loc } else { &id });
        }
        out
    }

    #[test]
    fn verify_bb40030_legacy_ad_structure() {
        let (id, loc) = messages();
        let mut decoder = BleDecoder::default();
        assert!(decoder.ingest(&ad(&id, 7), [1; 6], -40, BT4).unwrap().pos.is_none());
        let got = decoder.ingest(&ad(&loc, 8), [1; 6], -41, BT4).unwrap();
        assert_eq!(got.drone_id, "TEST");
        assert!((got.pos.as_ref().unwrap().lat - 40.8817203).abs() < 1e-7);
        assert_eq!(got.transport, Some(RidTransport::Bt4Legacy));
        assert_eq!(got.message_counter, Some(8));
        assert_eq!(
            decode_single_message(&id).unwrap().hardware_serial.unwrap(),
            "TEST"
        );
    }

    #[test]
    fn verify_bb40030_rejects_wrong_uuid_or_appcode() {
        for index in [2, 3, 4] {
            let mut bytes = ad(&messages().1, 0);
            bytes[index] ^= 1;
            assert!(ingest_ad(&bytes, [0; 6], -50, BT4).is_none());
        }
    }

    #[test]
    fn verify_bb50110_secondary_pack_decodes() {
        let got = ingest_ad(&ad(&pack(2), 9), [0; 6], -50, BT5).unwrap();
        assert_eq!(got.drone_id, "TEST");
        assert_eq!(got.transport, Some(RidTransport::Bt5LongRange));
    }

    #[test]
    fn verify_bb50120_pack_max_nine() {
        assert!(ingest_ad(&ad(&pack(9), 0), [0; 6], -50, BT5).is_some());
        let mut overclaim = pack(9);
        overclaim[2] = 10;
        assert_eq!(
            ingest_ad_result(&ad(&overclaim, 0), [0; 6], -50, BT5).unwrap_err(),
            Reject::PackCount
        );
        assert!(ingest_ad(&ad(&[0xf2, 25, 9], 0), [0; 6], -50, BT5).is_none());
    }

    #[test]
    fn verify_bur0050_message_counter_surfaces() {
        for counter in 0..=255 {
            assert_eq!(
                ingest_ad(&ad(&pack(2), counter), [0; 6], -50, BT5)
                    .unwrap()
                    .message_counter,
                Some(counter)
            );
        }
    }

    #[test]
    fn rejects_truncation_prefix_and_wrong_phy_without_panicking() {
        let good = ad(&pack(2), 0);
        for end in 0..good.len() {
            assert!(ingest_ad(&good[..end], [0; 6], -50, BT5).is_none());
        }
        let mut prefixed = vec![0];
        prefixed.extend(pack(2));
        assert!(ingest_ad(&ad(&prefixed, 0), [0; 6], -50, BT5).is_none());
        assert!(ingest_ad(
            &good,
            [0; 6],
            -50,
            BleMeta {
                phy: BlePhy::Le1M,
                extended: true
            }
        )
        .is_none());
    }

    #[test]
    fn bt4_identity_is_address_scoped_and_expires_without_replaying_position() {
        let (id, loc) = messages();
        let mut decoder = BleDecoder::default();
        let now = Instant::now();
        assert!(decoder
            .ingest_at(&ad(&id, 0), [1; 6], -40, BT4, now)
            .unwrap().pos.is_none());
        assert!(decoder.ingest_at(&ad(&loc, 0), [2; 6], -40, BT4, now).is_none());
        assert!(decoder.ingest_at(&ad(&loc, 0), [1; 6], -40, BT4, now + Duration::from_secs(60)).is_none());
        assert_eq!(decoder.take_unassociated_dropped(), 2);
        assert_eq!(decoder.take_unassociated_dropped(), 0);
        assert!(decoder
                .ingest_at(&ad(&id, 0), [1; 6], -40, BT4, now + Duration::from_secs(61))
            .unwrap().pos.is_none());
    }

    #[test]
    fn synthetic_two_macs_location_before_identity_cross_phy_and_ttl() {
        // Authored normalized JSON, NOT captured RF or customer data. Re-encode
        // the invented fields as a minimal ODID regression fixture.
        let frames: Vec<crate::models::NormalizedTelemetry> = include_str!(
            "../tests/fixtures/synthetic-ble-session.jsonl"
        ).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let find = |mac, known| frames.iter().find(|f| f.data.mac_address == mac && (f.data.drone_id != "UNKNOWN") == known).unwrap().data.clone();
        let a = [0x02, 0, 0, 0, 0, 1];
        let b = [0x02, 0, 0, 0, 0, 2];
        let location = |data: &TelemetryData| {
            let mut loc = [0u8; 25];
            loc[0] = 0x12;
            let pos = data.pos.as_ref().expect("fixture has a position");
            loc[5..9].copy_from_slice(&((pos.lat * 1e7).round() as i32).to_le_bytes());
            loc[9..13].copy_from_slice(&((pos.lon * 1e7).round() as i32).to_le_bytes());
            let alt_raw = pos.alt_m.map_or(0, |alt| ((alt + 1000.0) * 2.0) as u16);
            loc[15..17].copy_from_slice(&alt_raw.to_le_bytes());
            loc
        };
        let basic = |data: &TelemetryData| {
            let mut id = [0u8; 25];
            id[0] = 2; id[1] = 0x10;
            id[2..2 + data.drone_id.len()].copy_from_slice(data.drone_id.as_bytes());
            id
        };
        let (a_unknown, b_unknown) = (find(a, false), find(b, false));
        let (a_known, b_known) = (find(a, true), find(b, true));
        assert_eq!(a_known.drone_id, "SYNTHETIC-A");
        assert_eq!(b_known.drone_id, "SYNTHETIC-B");
        let mut decoder = BleDecoder::default();
        let now = Instant::now();
        let a_loc = ad(&location(&a_unknown), a_unknown.message_counter.unwrap());
        let b_loc = ad(&location(&b_unknown), b_unknown.message_counter.unwrap());
        assert!(decoder.ingest_at(&a_loc, a, -90, BT4, now).is_none());
        assert!(decoder.ingest_at(&b_loc, b, -88, BT4, now).is_none());
        let mut lr = vec![0xf2, 25, 2];
        lr.extend(basic(&a_known)); lr.extend(location(&a_known));
        assert_eq!(decoder.ingest_at(&ad(&lr, 255), a, -88, BT5, now).unwrap().drone_id, "SYNTHETIC-A");
        // A's LR identity cannot associate B's legacy location.
        assert!(decoder.ingest_at(&b_loc, b, -88, BT4, now).is_none());
        // Later Basic ID alone never replays either held/dropped position.
        assert!(decoder.ingest_at(&ad(&basic(&b_known), 20), b, -86, BT4, now).unwrap().pos.is_none());
        for (mac, loc, serial) in [(a, &a_loc, "SYNTHETIC-A"), (b, &b_loc, "SYNTHETIC-B")] {
            let got = decoder.ingest_at(loc, mac, -86, BT4, now + Duration::from_secs(59)).unwrap();
            assert_eq!(got.drone_id, serial);
            assert_eq!(got.hardware_serial.as_deref(), Some(serial));
        }
        for (mac, loc) in [(a, &a_loc), (b, &b_loc)] {
            assert!(decoder.ingest_at(loc, mac, -86, BT4, now + Duration::from_secs(60)).is_none());
        }
        assert_eq!(decoder.take_unassociated_dropped(), 5);
    }

    #[test]
    fn synthetic_shared_station_and_zero_operator_survive_decoding() {
        let rows: Vec<crate::models::NormalizedTelemetry> = include_str!(
            "../tests/fixtures/synthetic-ble-session.jsonl"
        ).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let mut station = None;
        let mut serials = std::collections::HashSet::new();
        for row in rows.iter().filter(|r| matches!(r.data.transport, Some(RidTransport::Bt5LongRange))) {
            let data = &row.data;
            let mut basic = [0u8;25]; basic[0]=2; basic[1]=0x10;
            basic[2..2+data.drone_id.len()].copy_from_slice(data.drone_id.as_bytes());
            let op = data.operator_pos.as_ref().unwrap();
            let mut system = [0u8;25]; system[0]=0x42;
            system[2..6].copy_from_slice(&((op.lat*1e7).round() as i32).to_le_bytes());
            system[6..10].copy_from_slice(&((op.lon*1e7).round() as i32).to_le_bytes());
            system[18..20].copy_from_slice(&2000u16.to_le_bytes());
            let mut operator = [0u8;25]; operator[0]=0x52; operator[2]=b'0';
            let mut pack = vec![0xf2,25,3];
            pack.extend(basic); pack.extend(system); pack.extend(operator);
            let got = ingest_ad(&ad(&pack,255),data.mac_address,-50,BT5).unwrap();
            assert_eq!(got.operator_id.as_deref(),Some("0"));
            let position = got.operator_pos.unwrap();
            assert_eq!((position.lat,position.lon,position.alt_m),(op.lat,op.lon,Some(0.0)));
            let current=(position.lat,position.lon);
            if let Some(prior)=station { assert_eq!(prior,current); }
            station=Some(current);serials.insert(got.drone_id);
        }
        assert_eq!(serials.len(),2);
    }

    #[test]
    fn cache_keeps_id_type_and_is_bounded() {
        let (mut id, loc) = messages();
        id[1] = 0x20; // CAA, not hardware serial.
        let mut decoder = BleDecoder::default();
        let now = Instant::now();
        for n in 0..129 {
            decoder.ingest_at(&ad(&id, 0), [n; 6], -40, BT4, now + Duration::from_millis(u64::from(n)));
        }
        assert_eq!(decoder.identities.len(), 128);
        assert!(!decoder.identities.contains_key(&[0; 6]));
        let got = decoder.ingest_at(&ad(&loc, 1), [128; 6], -40, BT4, now + Duration::from_secs(1)).unwrap();
        assert_eq!(got.caa_registration.as_deref(), Some("TEST"));
        assert_eq!(got.hardware_serial, None);
    }

    #[test]
    fn known_then_unknown_position_never_reuses_cached_fix() {
        let (id, mut loc) = messages();
        let mut decoder = BleDecoder::default();
        let now = Instant::now();
        let identity = decoder.ingest_at(&ad(&id, 0), [1; 6], -40, BT4, now).unwrap();
        assert!(identity.pos.is_none());
        let known = decoder.ingest_at(&ad(&loc, 1), [1; 6], -40, BT4, now).unwrap();
        assert!(known.pos.is_some());
        loc[5..13].fill(0);
        loc[1] = 2 << 4; // airborne claim cannot make a zero pair a valid fix
        let unknown = decoder.ingest_at(&ad(&loc, 2), [1; 6], -40, BT4, now).unwrap();
        assert!(unknown.pos.is_none());
        assert_eq!(unknown.drone_id, "TEST");
        assert_eq!(unknown.operational_status, Some(2));
        assert_eq!(unknown.position_unknown_reason, Some(crate::models::PositionUnknownReason::ZeroPair));
        assert!(!serde_json::to_value(unknown).unwrap().as_object().unwrap().contains_key("pos"));
    }

    #[test]
    fn wire_optional_metadata_is_backward_compatible() {
        let mut got = ingest_ad(&ad(&pack(2), 1), [0; 6], -50, BT5).unwrap();
        let mut value = serde_json::to_value(&got).unwrap();
        assert_eq!(value["transport"], "bt5_long_range");
        value.as_object_mut().unwrap().remove("transport");
        value.as_object_mut().unwrap().remove("message_counter");
        let old: TelemetryData = serde_json::from_value(value).unwrap();
        assert_eq!(old.transport, None);
        got.transport = None;
        got.message_counter = None;
        assert!(!serde_json::to_string(&got).unwrap().contains("transport"));
    }

    #[test]
    fn verify_bb50030_lr_vector_decodes() {
        // SHA256 586cd7ec4b7f6e9d9d13fafdf4573e9e9a39b8a57800c7939921e1936158b46b.
        // Nordic v3, linktype 272 (NOT LE-LL-PHDR 256). Wrapper walk only;
        // all ODID payloads enter ingest_ad_result and the shared core.
        let vector =
            include_bytes!("../../../../Standards/F3411/vectors/odid_bt5_lr_sample.pcapng");
        let u32le = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()) as usize;
        let mut offset = 0;
        let (mut epbs, mut aux, mut matches, mut decoded, mut invalid_capture) = (0, 0, 0, 0, 0);
        let mut reasons = std::collections::BTreeMap::new();
        let mut first = None;
        while offset < vector.len() {
            let kind = u32le(&vector[offset..offset + 4]);
            let size = u32le(&vector[offset + 4..offset + 8]);
            assert!(size >= 12 && offset + size <= vector.len());
            assert_eq!(u32le(&vector[offset + size - 4..offset + size]), size);
            if kind == 1 {
                assert_eq!(&vector[offset + 8..offset + 10], &[0x10, 1]);
            }
            if kind == 6 {
                epbs += 1;
                let caplen = u32le(&vector[offset + 20..offset + 24]);
                assert_eq!(caplen, 271);
                let p = &vector[offset + 28..offset + 28 + caplen];
                assert_eq!((p[3], p[7]), (3, 10));
                if p[6] != 2 || p[8] & 1 == 0 {
                    invalid_capture += 1;
                    offset += size;
                    continue;
                }
                assert_eq!((p[8] >> 4) & 7, 2); // LE Coded
                assert!(p[9] < 37);
                assert_eq!((p[8] >> 1) & 3, 0); // AUX_ADV_IND
                assert_eq!(&p[17..21], &[0xd6, 0xbe, 0x89, 0x8e]);
                assert_eq!(p[21], 0); // S=8 coding indication
                assert_eq!(p[22] & 15, 7);
                aux += 1;
                let body = &p[24..24 + usize::from(p[23])];
                let extlen = usize::from(body[0] & 63);
                let bytes = &body[extlen + 1..];
                assert!(bytes[1..].starts_with(&[0x16, 0xfa, 0xff, 0x0d]));
                matches += 1;
                let mut addr: [u8; 6] = body[2..8].try_into().unwrap();
                addr.reverse();
                match ingest_ad_result(bytes, addr, -i32::from(p[10]), BT5) {
                    Ok(data) => {
                        decoded += 1;
                        if first.is_none() {
                            first = Some(data);
                        }
                    }
                    Err(reason) => *reasons.entry(format!("{reason:?}")).or_insert(0) += 1,
                }
            }
            offset += size;
        }
        println!("VECTOR epb={epbs} aux={aux} signatures={matches} decoded={decoded} capture_rejected={invalid_capture} rejects={reasons:?}");
        let first = first.expect("STOP §8: no real vector frame decoded");
        println!(
            "FIRST_VECTOR_FRAME={}",
            serde_json::to_string(&first).unwrap()
        );
        assert_eq!((epbs, aux, matches, invalid_capture), (274, 244, 244, 30));
        assert_eq!(decoded + reasons.values().sum::<usize>(), matches);
    }
}
