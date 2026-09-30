// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use serde::{Deserialize, Serialize};

/// Wire-format version of the canonical telemetry payload.
///
/// History:
///   v1 — original flat shape (pre-Option β).
///   v2 — Option β refactor (Wave 6.0a, 2026-04-28): node + data blocks.
///   v3 — ASTM completeness (Wave 6.0c, 2026-04-29): added optional
///        `data.operator_id`, `data.self_id`, and `data.auth` fields.
///        Additive change at the JSON layer (skip_serializing_if = None);
///        v2 consumers ignore the new fields, v3 consumers see them.
///   v4 — unknown GeoPoint altitude is omitted (never JSON null); valid
///        2D fixes survive. Breaking contract: upgrade consumers first.
///        D27 amendment: unknown positions omitted; received status retained.
pub const WIRE_FORMAT_VERSION: u32 = 4;

/// Source protocol of a captured Remote-ID frame.
///
/// Wire-format names use the canonical ASTM/standards form (`ASTM_F3411_22a`
/// rather than the Rust-idiomatic camelCase) so JSON consumers (deck.gl
/// front-end, Databricks pipelines, downstream analytics) match the
/// official spec naming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolType {
    #[serde(rename = "ASTM_F3411_22a")]
    AstmF3411_22a,
    #[serde(rename = "DJI_Legacy_Wifi")]
    DjiLegacyWifi,
    #[serde(rename = "Unknown")]
    Unknown(Vec<u8>),
}

/// A geographic point used for both drone and operator positions.
///
/// Field names match the Option β consensus shape: short, JSON-compact,
/// matches the front-end deck.gl/H3 convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    /// Metres MSL when known. None means the source declared altitude unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_m: Option<f32>,
}

/// The static identity of the BRRDfeeder node that captured the frame.
///
/// Per consensus design 2026-04-28, every frame on the wire carries this
/// block so downstream can:
///   - trilaterate when 3+ feeders see the same drone (mac correlation +
///     RSSI from each feeder + node.location of each feeder)
///   - render coverage maps
///   - attribute telemetry back to a registered owner via node.id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLocation {
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
    /// Wave 7.4 (2026-05-26) — **position-truth attestation**.
    ///
    /// Carries forensic provenance for the lat/lon/alt values:
    ///   - `Some(PositionSource::GpsLive)`     — anchored to live GPS
    ///                                            fix; reputation-eligible.
    ///   - `Some(PositionSource::ConfigStatic)`— operator opted out of
    ///                                            GPS anchoring; sentinel
    ///                                            is self-disclosing as
    ///                                            UNTRUSTED. Globe-backend
    ///                                            Reputation Gravity should
    ///                                            refuse trust accumulation
    ///                                            against frames with this
    ///                                            source.
    ///   - `None`                              — pre-Wave-7.4 engine
    ///                                            (legacy frame; provenance
    ///                                            cannot be determined).
    ///
    /// Wire-format extension policy: additive, `skip_serializing_if =
    /// "Option::is_none"`. Pre-Wave-7.4 consumers ignore the new field;
    /// Wave-7.4+ consumers use it for trust-tier gating.
    ///
    /// Why this lives on NodeLocation (not on Node or NormalizedTelemetry):
    /// the attestation IS *about* the location data — when the operator
    /// reads back a frame, the position_source travels with the very
    /// lat/lon it qualifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_source: Option<PositionSource>,
}

/// Wave 7.4 — provenance enum for `NodeLocation`. Per the release approver's
/// "Reputation-Portability Attack" analysis 2026-05-26, the lat/lon
/// values in NodeLocation are meaningless to downstream trust
/// accumulation unless tagged with their source. A sensor that gains
/// reputation at site X and is then physically relocated to site Y
/// must declare the new position truthfully (`GpsLive` from satellite
/// consensus) or admit it's running un-anchored (`ConfigStatic`).
///
/// Snake_case wire serialization matches the convention used by
/// SensorState, RadioStatus, CoverageClass — all sentinel-tier enums.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PositionSource {
    /// Position anchored to live GPS-receiver consensus (NMEA quality >= 1,
    /// fix freshness within engine's stale_after_secs threshold).
    /// Reputation-eligible — globe-backend Reputation Gravity accumulates
    /// trust against frames carrying this source.
    GpsLive,
    /// Position is the operator's install-time config value, not anchored
    /// to a satellite-truth fix. Either:
    ///   - operator explicitly set `sensors.gps.required: false`, OR
    ///   - GPS sensor went stale (no fresh fix within stale_after_secs)
    ///     and the stamper fell back to config-static.
    /// Reputation-INELIGIBLE — globe-backend Reputation Gravity refuses
    /// trust accumulation against frames carrying this source.
    ConfigStatic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// BRRDfeeder node identifier (e.g., "bf-cm4-alpha-001"); set in config.yaml.
    pub id: String,
    /// Node's static physical location (set at install time in config.yaml).
    pub location: NodeLocation,
    /// BRRDfeeder release version (e.g., "0.8.20-ce").
    pub version: String,
}

/// Operator's free-text description of the flight purpose (ASTM Type 3).
/// Surfaced separately from operator_id because they're orthogonal: an
/// operator can broadcast a flight description without identifying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfIdInfo {
    /// 0=Text (general purpose), 1=Emergency, 2=Extended Status.
    pub desc_type: u8,
    /// Free-text description, UTF-8, up to 23 bytes, trimmed.
    pub description: String,
}

/// Authentication-message metadata (ASTM Type 2).
///
/// **CE-conservative scope**: we surface that authentication WAS broadcast
/// (auth_type, timestamp, total length) but do NOT reassemble multipart
/// auth_data across multiple Message Packs. Multipart reassembly +
/// signature verification belongs in BRRDfeeder-Pro / commercial
/// compliance flows where the full auth_data needs preserving for forensic
/// signature checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthInfo {
    /// AuthType: 0=None, 1=UAS-Id-Sig, 2=OperatorId-Sig, 3=Message-Set-Sig,
    /// 4=Network-Remote-ID, 5=Specific-Auth, 0xA-0xF=Private-Use.
    pub auth_type: u8,
    /// Timestamp from auth page 0, converted from the ASTM epoch
    /// (2019-01-01 00:00:00 UTC) to standard unix epoch. Present only when
    /// page 0 was seen in the captured Message Pack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_utc: Option<u64>,
    /// Total auth-data length declared on page 0 (bytes across all pages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_length: Option<u8>,
    /// Last-page index from page 0 (so total pages = last_page_index + 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_page_index: Option<u8>,
}

/// Per-frame drone telemetry, separated from the node identity so node info
/// is added once at the egress boundary rather than threaded through every
/// parser call.
///
/// New optional fields (operator_id, self_id, auth) added in Wave 6.0c
/// with `skip_serializing_if = None`: when absent, they don't show on the
/// wire; v2 consumers see no change, v3 consumers gain richer telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RidTransport {
    WifiBeacon,
    WifiNan,
    Bt4Legacy,
    Bt5LongRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<RidTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_counter: Option<u8>,
    pub protocol: ProtocolType,
    /// MAC address of the broadcasting drone (canonical hex form: aa:bb:cc:dd:ee:ff).
    #[serde(with = "hex_mac")]
    pub mac_address: [u8; 6],
    /// Primary UAS ID from ASTM Message Type 0 (BASIC_ID), trimmed.
    ///
    /// **Back-compat / convenience field** (kept so existing consumers —
    /// dedup, globe-backend, lake-writer — keep working unchanged): resolves
    /// to `hardware_serial` if present, else `caa_registration`, else
    /// `"UNKNOWN"`. For forensic work prefer the typed fields below, which
    /// distinguish a manufacturer serial from a CAA/FAA registration.
    pub drone_id: String,
    /// Manufacturer serial (ASTM BASIC_ID IDType 1, CTA-2063-A). Wave 8.x
    /// (#168) dual-ID split: distinct from `caa_registration` so a drone
    /// broadcasting both never overwrites one with the other (the Memorial
    /// Park forensic gap). Additive wire field; pre-#168 consumers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_serial: Option<String>,
    /// CAA/FAA-assigned registration ID (ASTM BASIC_ID IDType 2). Broadcast
    /// by Part-107 / enterprise operators; distinct from the airframe serial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caa_registration: Option<String>,
    /// Received LOCATION status nibble: 0 undeclared, 1 ground, 2 airborne,
    /// 3 emergency, 4 RID failure, 5..15 reserved. Never assumed truthful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operational_status: Option<u8>,
    /// Decoder inference, explicitly separate from the transmitter's claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_unknown_reason: Option<PositionUnknownReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_position_unknown_reason: Option<PositionUnknownReason>,
    /// Drone's position from ASTM LOCATION; omit when unknown/implausible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<GeoPoint>,
    /// Operator's position from ASTM Message Type 4 (SYSTEM), if broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_pos: Option<GeoPoint>,
    /// Operator registration ID from ASTM Message Type 5 (OPERATOR_ID).
    /// E.g., FAA-issued operator ID. Distinct from the drone serial in
    /// `drone_id`, which identifies the airframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_id: Option<String>,
    /// Operator's free-text description of the flight (ASTM Type 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_id: Option<SelfIdInfo>,
    /// Authentication metadata (ASTM Type 2). Page-0 fields only; CE does
    /// not reassemble multipart auth_data — see AuthInfo doc-comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthInfo>,
    /// Receive signal strength at the capturing feeder, from radiotap.
    pub signal_rssi_dbm: i32,
}

/// The canonical wire-format payload published to NATS subject
/// `cybrrd.telemetry.frame.rid.<node_id>`.
///
/// `timestamp_utc` unit: **Unix milliseconds** since epoch (Wave 6.4.1
/// substrate-truth lock 2026-05-03). The 10 Hz Virilio Spool cadence
/// is incompatible with seconds-resolution timestamps — same-second
/// frames produce Δt=0 at the globe-backend's Bohr Spool spacetime
/// gate, generating spurious TEMPORAL_INVERSION rejections.
///
/// Distinct from `AuthInfo.timestamp_utc` (above) which is the
/// drone's auth-page-0 timestamp parsed FROM the broadcast and
/// remains in seconds — that value is drone-sourced wire-format,
/// not feeder-emit-time, so its unit is whatever the drone supplies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedTelemetry {
    /// Format provenance, distinct from node.version (the sensor build).
    /// Absence remains unversioned: early v4 as well as v3 omitted this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_format_version: Option<u32>,
    pub node: Node,
    pub timestamp_utc: u64,
    pub data: TelemetryData,
}

/// Inferred from observed bytes, never a declaration of why GNSS is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionUnknownReason {
    ZeroPair,
    OutOfRange,
    NoLocation,
}

impl From<cybrrd_rid_core::PositionUnknownReason> for PositionUnknownReason {
    fn from(reason: cybrrd_rid_core::PositionUnknownReason) -> Self {
        use cybrrd_rid_core::PositionUnknownReason as Core;
        match reason {
            Core::ZeroPair => Self::ZeroPair,
            Core::OutOfRange => Self::OutOfRange,
            Core::NoLocation => Self::NoLocation,
        }
    }
}

/// Custom MAC <-> hex-string serde so JSON payloads carry the canonical
/// "aa:bb:cc:dd:ee:ff" form rather than a u8 array.
mod hex_mac {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(mac: &[u8; 6], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 6], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let mut mac = [0u8; 6];
        let bytes: Vec<u8> = s
            .split(':')
            .filter_map(|b| u8::from_str_radix(b, 16).ok())
            .collect();
        if bytes.len() == 6 {
            mac.copy_from_slice(&bytes[..6]);
            Ok(mac)
        } else {
            Err(serde::de::Error::custom("invalid MAC address format"))
        }
    }
}
