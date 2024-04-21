//! Classifying IP addresses for server-side request forgery protection.
//!
//! A reference in a schema is attacker-controlled: without a check, an
//! allowlisted host name that resolves to `127.0.0.1` or `169.254.169.254`
//! turns Faultline into a client for the CI runner's internal services.
//! Only globally routable unicast addresses may be connected to.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether connecting to `ip` reaches the public internet, as opposed to
/// the local host, a private network, or a reserved range.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let reserved = a == 0                              // "this network"
        || a == 10                                     // private
        || a == 127                                    // loopback
        || (a == 100 && (64..128).contains(&b))        // carrier-grade NAT
        || (a == 169 && b == 254)                      // link-local, cloud metadata
        || (a == 172 && (16..32).contains(&b))         // private
        || (a == 192 && b == 0 && c == 0)              // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)              // documentation
        || (a == 192 && b == 88 && c == 99)            // 6to4 relay anycast
        || (a == 192 && b == 168)                      // private
        || (a == 198 && (b == 18 || b == 19))          // benchmarking
        || (a == 198 && b == 51 && c == 100)           // documentation
        || (a == 203 && b == 0 && c == 113)            // documentation
        || a >= 224; // multicast, reserved, broadcast
    !reserved
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    // Addresses that embed an IPv4 address are judged by it: otherwise
    // `::ffff:127.0.0.1` would reach the loopback interface.
    if let Some(v4) = embedded_v4(ip) {
        return is_public_v4(v4);
    }
    let segments = ip.segments();
    let first = segments[0];
    let reserved = ip.is_unspecified()
        || ip.is_loopback()
        || (first & 0xfe00) == 0xfc00                  // unique local
        || (first & 0xffc0) == 0xfe80                  // link-local
        || (first & 0xffc0) == 0xfec0                  // site-local (deprecated)
        || (first & 0xff00) == 0xff00                  // multicast
        || (first == 0x2001 && segments[1] == 0x0db8)  // documentation
        || (first == 0x2001 && segments[1] < 0x0200)   // IETF protocol assignments, incl. Teredo
        || (first == 0x0100 && segments[1..4] == [0, 0, 0]) // discard-only
        || (first & 0xe000) != 0x2000; // outside global unicast 2000::/3
    !reserved
}

/// The IPv4 address carried by an IPv4-mapped (`::ffff:a.b.c.d`),
/// IPv4-compatible (`::a.b.c.d`), NAT64 (`64:ff9b::a.b.c.d`) or 6to4
/// (`2002:aabb:ccdd::`) address.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let tail = || {
        let [.., hi, lo] = s;
        Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo))
    };
    if s[..5] == [0; 5] && (s[5] == 0xffff || (s[5] == 0 && s[6] != 0)) {
        return Some(tail());
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return Some(tail());
    }
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::from((u32::from(s[1]) << 16) | u32::from(s[2])));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(s: &str) -> bool {
        is_public(s.parse().unwrap())
    }

    #[test]
    fn private_and_reserved_ipv4_ranges_are_refused() {
        for ip in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "127.255.255.254",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!public(ip), "{ip} should be refused");
        }
    }

    #[test]
    fn global_ipv4_addresses_are_allowed() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "93.184.216.34",
            "223.255.255.254",
        ] {
            assert!(public(ip), "{ip} should be allowed");
        }
    }

    #[test]
    fn private_and_reserved_ipv6_ranges_are_refused() {
        for ip in [
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "2001::1",
            "100::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2002:c0a8:0101::1",
            "4000::1",
        ] {
            assert!(!public(ip), "{ip} should be refused");
        }
    }

    #[test]
    fn global_ipv6_addresses_are_allowed() {
        for ip in [
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
        ] {
            assert!(public(ip), "{ip} should be allowed");
        }
    }
}
