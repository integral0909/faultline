//! Which URLs may be fetched.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use faultline_doc::UriRef;

use crate::address::is_public;

/// A host on the allowlist: an exact name (`schemas.example.com`) or every
/// subdomain of one (`*.example.com`, which does not match `example.com`
/// itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPattern {
    suffix: String,
    wildcard: bool,
}

impl HostPattern {
    /// Parses a pattern. Names are compared case-insensitively.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        let (wildcard, name) = match pattern.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        let valid = !name.is_empty()
            && name.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
        if !valid {
            return Err(format!(
                "{pattern:?} is not a host name or a *.domain pattern"
            ));
        }
        Ok(Self {
            suffix: name.to_ascii_lowercase(),
            wildcard,
        })
    }

    pub fn matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if self.wildcard {
            host.len() > self.suffix.len()
                && host.ends_with(&self.suffix)
                && host.as_bytes()[host.len() - self.suffix.len() - 1] == b'.'
        } else {
            host == self.suffix
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.wildcard {
            f.write_str("*.")?;
        }
        f.write_str(&self.suffix)
    }
}

/// The rules remote fetching follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkPolicy {
    /// Hosts that may be contacted. Empty means none.
    pub allowed_hosts: Vec<HostPattern>,
    /// Largest response body accepted.
    pub max_bytes: u64,
    /// Limit on each request, from connecting to reading the body.
    pub timeout: Duration,
    /// Redirects followed per document.
    pub max_redirects: u32,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            allowed_hosts: Vec::new(),
            max_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(20),
            max_redirects: 5,
        }
    }
}

/// A URL that passed the policy's static checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl NetworkPolicy {
    /// Checks everything about `url` that can be checked without resolving
    /// it: scheme, credentials, host and port. Addresses are checked when
    /// connecting.
    pub fn check(&self, url: &UriRef) -> Result<Target, String> {
        match url.scheme() {
            Some(s) if s.eq_ignore_ascii_case("https") => {}
            Some(s) if s.eq_ignore_ascii_case("http") => {
                return Err(format!("{url} uses plain HTTP; only HTTPS is fetched"));
            }
            _ => return Err(format!("{url} is not an https URL")),
        }
        let authority = url
            .authority()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| format!("{url} has no host"))?;
        if authority.contains('@') {
            return Err(format!("{url} contains credentials, which are never sent"));
        }
        let (host, port) = split_host_port(authority)
            .ok_or_else(|| format!("{url} has an invalid host or port"))?;
        let literal = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = literal.parse::<IpAddr>()
            && !is_public(ip)
        {
            return Err(format!("{url} names a non-public address"));
        }
        if !self.allowed_hosts.iter().any(|p| p.matches(host)) {
            return Err(format!("{host} is not in [network] allowed_hosts"));
        }
        Ok(Target {
            host: host.to_ascii_lowercase(),
            port,
        })
    }
}

/// Splits `host[:port]`, where host may be a bracketed IPv6 literal.
fn split_host_port(authority: &str) -> Option<(&str, u16)> {
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']')?;
        let (host, rest) = authority.split_at(end + 1);
        match rest {
            "" => (host, None),
            _ => (host, Some(rest.strip_prefix(':')?)),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        None | Some("") => 443,
        Some(p) => p.parse().ok().filter(|p| *p != 0)?,
    };
    (!host.is_empty()).then_some((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(hosts: &[&str]) -> NetworkPolicy {
        NetworkPolicy {
            allowed_hosts: hosts
                .iter()
                .map(|h| HostPattern::parse(h).unwrap())
                .collect(),
            ..NetworkPolicy::default()
        }
    }

    fn check(policy: &NetworkPolicy, url: &str) -> Result<Target, String> {
        policy.check(&UriRef::parse(url).unwrap())
    }

    #[test]
    fn host_patterns() {
        let exact = HostPattern::parse("Schemas.Example.com").unwrap();
        assert!(exact.matches("schemas.example.COM"));
        assert!(!exact.matches("evil-schemas.example.com"));
        let wildcard = HostPattern::parse("*.example.com").unwrap();
        assert!(wildcard.matches("a.example.com"));
        assert!(wildcard.matches("a.b.example.com"));
        assert!(!wildcard.matches("example.com"));
        assert!(!wildcard.matches("badexample.com"));
        assert_eq!(wildcard.to_string(), "*.example.com");
        for bad in ["", "*", "*.", "a..b", "exa mple.com", "*.*.com", "a/b"] {
            assert!(HostPattern::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_allowlisted_https_hosts_pass() {
        let p = policy(&["schemas.example.com", "*.json-schema.org"]);
        assert_eq!(
            check(&p, "https://schemas.example.com/a.json").unwrap(),
            Target {
                host: "schemas.example.com".into(),
                port: 443
            }
        );
        assert_eq!(
            check(&p, "https://www.json-schema.org:8443/x")
                .unwrap()
                .port,
            8443
        );
        for (url, reason) in [
            ("http://schemas.example.com/a.json", "plain HTTP"),
            ("ftp://schemas.example.com/a.json", "not an https URL"),
            (
                "https://other.example.com/a.json",
                "not in [network] allowed_hosts",
            ),
            ("https://user:pw@schemas.example.com/a.json", "credentials"),
            (
                "https://schemas.example.com:0/a.json",
                "invalid host or port",
            ),
            (
                "https://schemas.example.com:99999/a.json",
                "invalid host or port",
            ),
            ("https:///a.json", "no host"),
        ] {
            let err = check(&p, url).unwrap_err();
            assert!(err.contains(reason), "{url}: {err}");
        }
    }

    #[test]
    fn literal_private_addresses_are_refused_even_when_allowlisted() {
        let p = policy(&["127.0.0.1"]);
        assert!(
            check(&p, "https://127.0.0.1/a.json")
                .unwrap_err()
                .contains("non-public")
        );
        assert!(
            check(&p, "https://[::1]:8443/a.json")
                .unwrap_err()
                .contains("non-public")
        );
    }

    #[test]
    fn nothing_passes_an_empty_allowlist() {
        assert!(check(&NetworkPolicy::default(), "https://example.com/").is_err());
    }

    #[test]
    fn host_and_port_splitting() {
        assert_eq!(split_host_port("a.com"), Some(("a.com", 443)));
        assert_eq!(split_host_port("a.com:"), Some(("a.com", 443)));
        assert_eq!(split_host_port("a.com:8080"), Some(("a.com", 8080)));
        assert_eq!(
            split_host_port("[2001:db8::1]:9"),
            Some(("[2001:db8::1]", 9))
        );
        assert_eq!(
            split_host_port("[2001:db8::1]"),
            Some(("[2001:db8::1]", 443))
        );
        assert_eq!(split_host_port("[2001:db8::1]x"), None);
        assert_eq!(split_host_port(":80"), None);
    }
}
