//! Which addresses a page may be fetched from (issue 544): those of the public internet, not those
//! of the network the server runs in, so that a URL a model writes - or that a page it read told it
//! to write - reaches no service the server reaches and the internet does not: the server itself,
//! a host of its network, the metadata service of a cloud.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `address` lies on the public internet: not the host itself, nor a private, shared,
/// link-local, reserved, special-purpose or documentation network, nor a multicast group. An IPv6
/// address that carries an IPv4 one - mapped, translated by NAT64, or tunnelled by 6to4 - is
/// judged by that.
pub(super) fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match carried_v4(v6) {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    let this_network = first == 0;
    // RFC 6598, the shared address space of carrier-grade NAT: 100.64.0.0/10.
    let shared = first == 100 && (64..128).contains(&second);
    // RFC 1112, reserved for future use: 240.0.0.0/4, the broadcast address among them.
    let reserved = first >= 240;
    // RFC 6890, the assignments of the IETF's protocols: 192.0.0.0/24.
    let protocols = address.octets()[..3] == [192, 0, 0];
    // RFC 2544, for benchmarks: 198.18.0.0/15, which VPNs and resolvers of fake addresses use
    // inside networks.
    let benchmarks = first == 198 && (second & 0xfe) == 18;
    !(this_network
        || shared
        || protocols
        || benchmarks
        || reserved
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_documentation())
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let first = address.segments()[0];
    // RFC 4193, unique local addresses: fc00::/7.
    let unique_local = first & 0xfe00 == 0xfc00;
    // RFC 4291, link-local unicast: fe80::/10; and the site-local ones it deprecated: fec0::/10.
    let link_or_site_local = first & 0xff80 == 0xfe80;
    // RFC 3849, for documentation: 2001:db8::/32.
    let documentation = first == 0x2001 && address.segments()[1] == 0x0db8;
    // RFC 8215, the prefix of NAT64 for local use: 64:ff9b:1::/48, which carries an IPv4 address
    // in a place that depends on the length of the prefix a network chose.
    let local_nat64 = address.segments()[..3] == [0x64, 0xff9b, 1];
    !(unique_local
        || local_nat64
        || link_or_site_local
        || documentation
        || address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast())
}

/// The IPv4 address `address` carries: as an IPv4-mapped address (`::ffff:a.b.c.d`), in the
/// well-known prefix of NAT64 (`64:ff9b::a.b.c.d`, RFC 6052), through which a translator reaches
/// it, or after the prefix of 6to4 (`2002:aabb:ccdd::`, RFC 3056), through which a relay does.
fn carried_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    let octets = address.octets();
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return Some(Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]));
    }
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }
    address.to_ipv4_mapped()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(address: &str) -> bool {
        is_public(address.parse().unwrap())
    }

    #[test]
    fn the_public_internet_is_reached() {
        for address in ["93.184.215.14", "1.1.1.1", "100.128.0.1", "2a00:1450:4001::200e"] {
            assert!(public(address), "{address}");
        }
    }

    #[test]
    fn nothing_inside_the_network_is_reached() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.131.25",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.2.1",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.1",
            "::",
            "::1",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!public(address), "{address}");
        }
    }

    /// An IPv4 address of the network does not pass as IPv6.
    #[test]
    fn an_address_of_the_network_is_not_reached_in_ipv6_either() {
        for address in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::101:101",
            "2002:7f00:1::",
            "2002:a9fe:a9fe::1",
        ] {
            assert!(!public(address), "{address}");
        }
        assert!(public("::ffff:1.1.1.1"), "a public one is reached so");
        assert!(public("64:ff9b::101:101"), "or through NAT64");
        assert!(public("2002:101:101::1"), "or through 6to4");
    }
}
