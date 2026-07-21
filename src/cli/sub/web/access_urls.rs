use super::networks::{AddressFamily, NetworkAddress, format_host_for_url, list_network_addresses};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUrlLine {
    pub label: &'static str,
    pub url: String,
}

// Original:
//   apps/kimi-code/src/cli/sub/web/access-urls.ts
//   buildOpenableUrl()
pub fn build_openable_url(bare_origin: &str, token: Option<&str>) -> String {
    let base = bare_origin.strip_suffix('/').unwrap_or(bare_origin);
    match token {
        Some(token) => format!("{base}/#token={token}"),
        None => format!("{base}/"),
    }
}

// Original: splitTokenFragment()
pub fn split_token_fragment(full_url: &str) -> (&str, &str) {
    const MARKER: &str = "#token=";
    match full_url.find(MARKER) {
        Some(index) => full_url.split_at(index),
        None => (full_url, ""),
    }
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn is_wildcard(host: &str) -> bool {
    matches!(host, "" | "0.0.0.0" | "::")
}

fn host_origin(host: &str, port: u16) -> String {
    let family = if host.contains(':') {
        AddressFamily::Ipv6
    } else {
        AddressFamily::Ipv4
    };
    format!("http://{}:{port}", format_host_for_url(host, family))
}

// Original: accessUrlLines()
pub fn access_url_lines(
    host: &str,
    port: u16,
    token: Option<&str>,
    network_addresses: Option<&[NetworkAddress]>,
) -> Vec<AccessUrlLine> {
    if is_wildcard(host) {
        let mut lines = vec![AccessUrlLine {
            label: "Local:    ",
            url: build_openable_url(&format!("http://localhost:{port}"), token),
        }];
        let discovered;
        let addresses = match network_addresses {
            Some(addresses) => addresses,
            None => {
                discovered = list_network_addresses();
                &discovered
            }
        };
        lines.extend(addresses.iter().map(|address| AccessUrlLine {
            label: "Network:  ",
            url: build_openable_url(
                &format!(
                    "http://{}:{port}",
                    format_host_for_url(&address.address, address.family)
                ),
                token,
            ),
        }));
        return lines;
    }
    if is_loopback_host(host) {
        return vec![AccessUrlLine {
            label: "Local:    ",
            url: build_openable_url(&host_origin(host, port), token),
        }];
    }
    vec![AccessUrlLine {
        label: "URL:      ",
        url: build_openable_url(&host_origin(host, port), token),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls_with_tokens_only_in_the_fragment() {
        assert_eq!(
            build_openable_url("http://127.0.0.1:58627", Some("abc123")),
            "http://127.0.0.1:58627/#token=abc123"
        );
        assert_eq!(
            build_openable_url("http://127.0.0.1:58627/", Some("t")),
            "http://127.0.0.1:58627/#token=t"
        );
        assert_eq!(
            build_openable_url("http://127.0.0.1:58627", None),
            "http://127.0.0.1:58627/"
        );
    }

    #[test]
    fn splits_token_fragments_without_modifying_the_url() {
        assert_eq!(
            split_token_fragment("http://h:1/#token=abc"),
            ("http://h:1/", "#token=abc")
        );
        assert_eq!(split_token_fragment("http://h:1/"), ("http://h:1/", ""));
    }

    #[test]
    fn returns_local_and_network_lines_for_a_wildcard_bind() {
        let addresses = [NetworkAddress {
            address: "192.168.1.5".to_owned(),
            family: AddressFamily::Ipv4,
        }];
        assert_eq!(
            access_url_lines("0.0.0.0", 58_627, Some("tok"), Some(&addresses)),
            [
                AccessUrlLine {
                    label: "Local:    ",
                    url: "http://localhost:58627/#token=tok".to_owned(),
                },
                AccessUrlLine {
                    label: "Network:  ",
                    url: "http://192.168.1.5:58627/#token=tok".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn labels_loopback_specific_and_ipv6_hosts() {
        assert_eq!(
            access_url_lines("127.0.0.1", 58_627, Some("tok"), None),
            [AccessUrlLine {
                label: "Local:    ",
                url: "http://127.0.0.1:58627/#token=tok".to_owned(),
            }]
        );
        assert_eq!(
            access_url_lines("192.168.1.5", 58_627, None, None),
            [AccessUrlLine {
                label: "URL:      ",
                url: "http://192.168.1.5:58627/".to_owned(),
            }]
        );
        assert_eq!(
            access_url_lines("2001:db8::1", 58_627, None, None)[0].url,
            "http://[2001:db8::1]:58627/"
        );
    }
}
