// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
//! Device-boundary route guard (C5): the engine administers ONLY the
//! dedicated capture adapter. If that adapter carries the host's default
//! route, it is the uplink — administering it (monitor mode, channel
//! setting) would cut the host's networking. Fail closed BEFORE any radio
//! administration; never touch routes, DNS, firewall, other interfaces,
//! or the built-in Wi-Fi/BT.

/// Parse a `/proc/net/route` table (text) into (iface, destination)
/// pairs. Destination is the raw hex column ("00000000" for default).
pub fn parse_routes(proc_net_route: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in proc_net_route.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let (Some(iface), Some(dest)) = (cols.next(), cols.next()) else {
            continue;
        };
        if let Ok(d) = u32::from_str_radix(dest, 16) {
            out.push((iface.to_string(), d));
        }
    }
    out
}

/// True when `iface` carries a default route (destination 0.0.0.0/0).
pub fn iface_carries_default_route(proc_net_route: &str, iface: &str) -> bool {
    parse_routes(proc_net_route)
        .iter()
        .any(|(i, dest)| i == iface && *dest == 0)
}

/// Startup gate: refuse to run when the designated capture interface
/// carries the default route, or when the route table cannot be read
/// (fail closed: unverifiable means unadministerable).
pub fn ensure_capture_interface_is_not_default_route(iface: &str) -> Result<(), String> {
    let table = std::fs::read_to_string("/proc/net/route").map_err(|e| {
        format!(
            "capture interface {iface}: cannot read /proc/net/route ({e}); \
             failing closed — the device boundary requires verification"
        )
    })?;
    if iface_carries_default_route(&table, iface) {
        return Err(format!(
            "capture interface {iface} carries the default route — it is the host \
             uplink, not a dedicated capture adapter. Refusing to administer it \
             (device boundary). Configure a dedicated adapter for capture."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
        eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
        wlx00c0caa697b3\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n";

    #[test]
    fn default_route_on_capture_interface_is_detected() {
        assert!(iface_carries_default_route(FIXTURE, "wlx00c0caa697b3"));
    }

    #[test]
    fn non_default_interfaces_pass() {
        assert!(!iface_carries_default_route(FIXTURE, "eth0_uplink_absent"));
        // A capture adapter with only link-local routes passes even though
        // another interface carries the default route.
        let table = "Iface\tDestination\tGateway\tFlags\n\
            eth0\t00000000\t0100A8C0\t0003\n\
            wlxdeadbeef01\t0002A8C0\t00000000\t0001\n";
        assert!(!iface_carries_default_route(table, "wlxdeadbeef01"));
    }

    #[test]
    fn empty_or_malformed_table_has_no_false_positive() {
        assert!(!iface_carries_default_route("", "anything"));
        assert!(!iface_carries_default_route(
            "garbage\nno columns",
            "anything"
        ));
    }

    #[test]
    fn parse_routes_extracts_iface_and_destination() {
        let routes = parse_routes(FIXTURE);
        assert!(routes.contains(&("eth0".to_string(), 0)));
        assert!(routes.contains(&("eth0".to_string(), 0x0001A8C0)));
    }
}
