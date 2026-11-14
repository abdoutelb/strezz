//! Work out which address phones and other machines should use to reach the
//! coordinator. `localhost`, Docker bridges and VPN tunnels are useless to a
//! phone on Wi-Fi, so rank interfaces and prefer real LAN addresses.

use std::net::{IpAddr, Ipv4Addr};

/// Interface name prefixes that never lead to the physical LAN.
const VIRTUAL_IFACES: &[&str] = &[
    "lo",
    "docker",
    "br-",
    "veth",
    "virbr",
    "vbox",
    "vmnet",
    "utun",
    "tun",
    "tap",
    "awdl",
    "llw",
    "bridge",
    "anpi",
    "ap1",
    "gif",
    "stf",
    "zt",
    "tailscale",
    "wg",
    "cni",
    "flannel",
    "kube",
];

/// Interface name prefixes of typical Wi-Fi/Ethernet adapters.
const LAN_IFACES: &[&str] = &["en", "eth", "wlan", "wl", "Wi-Fi", "Ethernet"];

fn is_private(ip: &Ipv4Addr) -> bool {
    ip.is_private() // 10/8, 172.16/12, 192.168/16
}

/// Lower is better; `None` means the address is unusable from other devices.
fn score(name: &str, ip: &Ipv4Addr) -> Option<u8> {
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    if VIRTUAL_IFACES.iter().any(|p| name.starts_with(p)) {
        return None;
    }
    // Docker's default bridge range, even when the interface name is unusual.
    let docker_range = ip.octets()[0] == 172 && (17..=31).contains(&ip.octets()[1]);
    let lan_name = LAN_IFACES.iter().any(|p| name.starts_with(p));
    Some(
        match (is_private(ip) && !docker_range, lan_name) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2, // a public IP directly on the NIC
            (false, false) => 3,
        } * 2
            + u8::from(docker_range),
    )
}

/// Rank `(interface, ip)` pairs, best first, dropping unusable ones.
pub fn rank(ifaces: &[(String, IpAddr)]) -> Vec<Ipv4Addr> {
    let mut found: Vec<(u8, Ipv4Addr)> = ifaces
        .iter()
        .filter_map(|(name, ip)| match ip {
            IpAddr::V4(v4) => score(name, v4).map(|s| (s, *v4)),
            IpAddr::V6(_) => None,
        })
        .collect();
    found.sort();
    found.dedup_by_key(|(_, ip)| *ip);
    found.into_iter().map(|(_, ip)| ip).collect()
}

/// LAN addresses of this machine, best first.
pub fn lan_addresses() -> Vec<Ipv4Addr> {
    let mut ifaces = local_ip_address::list_afinet_netifas().unwrap_or_default();
    // Fall back to the OS default-route address if enumeration found nothing.
    if let Ok(ip) = local_ip_address::local_ip() {
        ifaces.push(("default".into(), ip));
    }
    rank(&ifaces)
}

/// True when running inside a container, where interface addresses are
/// internal and phones can't reach them.
pub fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn prefers_wifi_over_loopback_docker_and_vpn() {
        let ifaces = vec![
            ("lo0".to_string(), v4("127.0.0.1")),
            ("docker0".to_string(), v4("172.17.0.1")),
            ("utun3".to_string(), v4("10.8.0.2")),
            ("en0".to_string(), v4("192.168.1.4")),
            ("eth0".to_string(), v4("172.18.0.5")),
            ("en5".to_string(), v4("169.254.10.1")),
        ];
        let ranked = rank(&ifaces);
        assert_eq!(ranked[0], "192.168.1.4".parse::<Ipv4Addr>().unwrap());
        assert!(!ranked
            .iter()
            .any(|ip| ip.is_loopback() || ip.is_link_local()));
        assert!(!ranked.contains(&"172.17.0.1".parse().unwrap()));
        assert!(!ranked.contains(&"10.8.0.2".parse().unwrap()));
    }

    #[test]
    fn nothing_usable_gives_empty() {
        assert!(rank(&[("lo".into(), v4("127.0.0.1"))]).is_empty());
    }
}
