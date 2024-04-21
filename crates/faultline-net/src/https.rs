//! The HTTPS [`Transport`], built on `ureq` with rustls.

use std::fmt;
use std::net::SocketAddr;

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::address::is_public;
use crate::{FetchError, Fetched, NetworkPolicy, Transport};

/// Fetches over HTTPS, connecting only to public addresses.
///
/// Redirects are returned to the caller rather than followed, so that each
/// target is checked against the policy; proxies are not used, because the
/// address check must apply to the host actually contacted.
pub struct HttpsTransport {
    agent: ureq::Agent,
}

impl fmt::Debug for HttpsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsTransport").finish_non_exhaustive()
    }
}

impl HttpsTransport {
    pub fn new(policy: &NetworkPolicy) -> Self {
        let config = Config::builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .timeout_global(Some(policy.timeout))
            .user_agent(concat!("faultline/", env!("CARGO_PKG_VERSION")))
            // Compressed bodies would be decompressed past the size limit.
            .accept_encoding("identity")
            .build();
        let agent = ureq::Agent::with_parts(config, DefaultConnector::new(), PublicOnly::default());
        Self { agent }
    }
}

impl Transport for HttpsTransport {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Fetched, FetchError> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| fetch_error(e, max_bytes))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return match response.headers().get("location").map(|v| v.to_str()) {
                Some(Ok(location)) => Ok(Fetched::Redirect(location.to_owned())),
                _ => Err(FetchError::Failed(format!(
                    "HTTP {status} redirect without a usable Location header"
                ))),
            };
        }
        if status != 200 {
            return Ok(Fetched::Status(status));
        }
        response
            .body_mut()
            .with_config()
            .limit(max_bytes)
            .read_to_vec()
            .map(Fetched::Body)
            .map_err(|e| fetch_error(e, max_bytes))
    }
}

fn fetch_error(error: ureq::Error, max_bytes: u64) -> FetchError {
    match error {
        ureq::Error::Other(inner) => match inner.downcast::<Refusal>() {
            Ok(refusal) => FetchError::Refused(refusal.0),
            Err(other) => FetchError::Failed(other.to_string()),
        },
        ureq::Error::BodyExceedsLimit(_) => FetchError::TooLarge { limit: max_bytes },
        other => FetchError::Failed(other.to_string()),
    }
}

/// A connection the address check refused.
#[derive(Debug)]
struct Refusal(String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Resolves host names and refuses the connection if any address is not
/// public. Because the connection uses exactly the addresses checked here,
/// a DNS answer that changes between the check and the connection (DNS
/// rebinding) cannot redirect it.
#[derive(Debug, Default)]
struct PublicOnly {
    inner: DefaultResolver,
}

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addresses = self.inner.resolve(uri, config, timeout)?;
        let host = uri.host().unwrap_or_default();
        vet(host, &addresses).map_err(|reason| ureq::Error::Other(Box::new(Refusal(reason))))?;
        Ok(addresses)
    }
}

/// Refuses a host if any of its addresses is not public: a name with both a
/// public and a private address could otherwise be used to reach the
/// private one on a retry.
pub(crate) fn vet(host: &str, addresses: &[SocketAddr]) -> Result<(), String> {
    match addresses.iter().find(|a| !is_public(a.ip())) {
        Some(private) => Err(format!(
            "{host} resolves to the non-public address {}",
            private.ip()
        )),
        None if addresses.is_empty() => Err(format!("{host} did not resolve")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    #[test]
    fn any_private_address_refuses_the_host() {
        assert!(vet("a.example", &addrs(&["93.184.216.34:443"])).is_ok());
        let err = vet("a.example", &addrs(&["93.184.216.34:443", "10.0.0.5:443"])).unwrap_err();
        assert_eq!(err, "a.example resolves to the non-public address 10.0.0.5");
        assert!(vet("a.example", &[]).is_err());
    }

    #[test]
    fn loopback_hosts_are_refused_at_connection_time() {
        // `localhost` passes a policy that allowlists it, but must not be
        // connected to. No network is needed: resolution is local.
        let policy = NetworkPolicy::default();
        let transport = HttpsTransport::new(&policy);
        for url in [
            "https://localhost:9/schema.json",
            "https://127.0.0.1:9/schema.json",
        ] {
            match transport.get(url, 1024) {
                Err(FetchError::Refused(reason)) => {
                    assert!(reason.contains("non-public"), "{url}: {reason}");
                }
                other => panic!("{url}: expected a refusal, got {other:?}"),
            }
        }
    }
}
