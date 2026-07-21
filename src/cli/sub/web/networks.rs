use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAddress {
    pub address: String,
    pub family: AddressFamily,
}

// Original:
//   apps/kimi-code/src/cli/sub/web/networks.ts
//   listNetworkAddresses()
pub fn list_network_addresses() -> Vec<NetworkAddress> {
    let raw = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| {
            let ip = interface.ip();
            NetworkAddress {
                address: ip.to_string(),
                family: if ip.is_ipv4() {
                    AddressFamily::Ipv4
                } else {
                    AddressFamily::Ipv6
                },
            }
        })
        .collect::<Vec<_>>();
    filter_display_addresses(&raw)
}

// Original: filterDisplayAddresses()
pub fn filter_display_addresses(addresses: &[NetworkAddress]) -> Vec<NetworkAddress> {
    let mut seen = HashSet::new();
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    for address in addresses {
        if address.family == AddressFamily::Ipv6 && is_link_local_v6(&address.address) {
            continue;
        }
        if !seen.insert(address.address.clone()) {
            continue;
        }
        match address.family {
            AddressFamily::Ipv4 => ipv4.push(address.clone()),
            AddressFamily::Ipv6 => ipv6.push(address.clone()),
        }
    }
    ipv4.extend(ipv6);
    ipv4
}

fn is_link_local_v6(address: &str) -> bool {
    address
        .split(':')
        .next()
        .and_then(|first| u16::from_str_radix(first, 16).ok())
        .is_some_and(|first| (0xfe80..=0xfebf).contains(&first))
}

// Original: formatHostForUrl()
pub fn format_host_for_url(address: &str, family: AddressFamily) -> String {
    match family {
        AddressFamily::Ipv4 => address.to_owned(),
        AddressFamily::Ipv6 => format!("[{address}]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(address: &str, family: AddressFamily) -> NetworkAddress {
        NetworkAddress {
            address: address.to_owned(),
            family,
        }
    }

    #[test]
    fn filters_link_local_duplicates_and_orders_ipv4_first() {
        let result = filter_display_addresses(&[
            address("2001:db8::1", AddressFamily::Ipv6),
            address("192.168.1.5", AddressFamily::Ipv4),
            address("fe80::1", AddressFamily::Ipv6),
            address("febf::2", AddressFamily::Ipv6),
            address("fec0::1", AddressFamily::Ipv6),
            address("192.168.1.5", AddressFamily::Ipv4),
        ]);
        assert_eq!(
            result,
            [
                address("192.168.1.5", AddressFamily::Ipv4),
                address("2001:db8::1", AddressFamily::Ipv6),
                address("fec0::1", AddressFamily::Ipv6),
            ]
        );
    }

    #[test]
    fn wraps_only_ipv6_hosts_for_urls() {
        assert_eq!(
            format_host_for_url("192.168.1.5", AddressFamily::Ipv4),
            "192.168.1.5"
        );
        assert_eq!(
            format_host_for_url("2001:db8::1", AddressFamily::Ipv6),
            "[2001:db8::1]"
        );
    }
}
