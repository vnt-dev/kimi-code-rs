use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Network exposure tier implied by a server bind address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindClass {
    Loopback,
    Lan,
    Public,
}

/// Options for [`classify`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassifyOptions {
    /// Override wildcard bind classification. `Loopback` is deliberately not
    /// accepted because a wildcard bind can never be loopback-only.
    pub bind_class: Option<WildcardBindClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WildcardBindClass {
    Lan,
    Public,
}

impl From<WildcardBindClass> for BindClass {
    fn from(value: WildcardBindClass) -> Self {
        match value {
            WildcardBindClass::Lan => Self::Lan,
            WildcardBindClass::Public => Self::Public,
        }
    }
}

// Original:
//   packages/kap-server/src/security/bindClassify.ts
//   classify()
//
// Rust adaptation:
//   std::net performs literal parsing; classification order and conservative
//   treatment of non-IP hostnames remain unchanged.
pub fn classify(host: &str, options: ClassifyOptions) -> BindClass {
    if host.is_empty() || host == "0.0.0.0" || host == "::" {
        return options
            .bind_class
            .map(BindClass::from)
            .unwrap_or(BindClass::Public);
    }
    if host == "localhost" {
        return BindClass::Loopback;
    }

    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if ip.is_loopback() => BindClass::Loopback,
        Ok(IpAddr::V4(ip)) if is_lan_v4(ip) => BindClass::Lan,
        Ok(IpAddr::V4(_)) => BindClass::Public,
        Ok(IpAddr::V6(ip)) if ip == Ipv6Addr::LOCALHOST => BindClass::Loopback,
        Ok(IpAddr::V6(ip)) if is_link_local_v6(ip) => BindClass::Lan,
        Ok(IpAddr::V6(_)) | Err(_) => BindClass::Public,
    }
}

fn is_lan_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
}

fn is_link_local_v6(ip: Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_loopback_hosts() {
        for host in ["127.0.0.1", "127.255.255.255", "::1", "localhost"] {
            assert_eq!(
                classify(host, ClassifyOptions::default()),
                BindClass::Loopback
            );
        }
    }

    #[test]
    fn classifies_lan_hosts() {
        for host in [
            "192.168.1.5",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "169.254.1.1",
            "fe80::1",
            "fe80:0000:0000:0000:0000:0000:0000:0001",
            "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        ] {
            assert_eq!(classify(host, ClassifyOptions::default()), BindClass::Lan);
        }
    }

    #[test]
    fn classifies_public_hosts() {
        for host in [
            "8.8.8.8",
            "172.32.0.1",
            "203.0.113.5",
            "2001:4860:4860::8888",
            "fec0::1",
            "example.com",
        ] {
            assert_eq!(
                classify(host, ClassifyOptions::default()),
                BindClass::Public
            );
        }
    }

    #[test]
    fn wildcard_override_only_affects_wildcards() {
        let options = ClassifyOptions {
            bind_class: Some(WildcardBindClass::Lan),
        };
        assert_eq!(classify("", options), BindClass::Lan);
        assert_eq!(classify("0.0.0.0", options), BindClass::Lan);
        assert_eq!(classify("::", options), BindClass::Lan);
        assert_eq!(classify("127.0.0.1", options), BindClass::Loopback);
        assert_eq!(classify("192.168.1.5", options), BindClass::Lan);
    }
}
