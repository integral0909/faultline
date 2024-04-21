//! Fetching remote documents under a network policy.
//!
//! Remote references are off unless a workspace enables them, and when it
//! does, [`RemoteLoader`] only fetches what its [`NetworkPolicy`] permits:
//!
//! * HTTPS only, without credentials in the URL;
//! * hosts on an explicit allowlist;
//! * redirects followed one hop at a time, each target checked again;
//! * responses capped in size and time;
//! * connections only to globally routable addresses, checked after DNS
//!   resolution (see [`address`]), so an allowlisted name that resolves to
//!   a private address, and DNS rebinding, are both refused.
//!
//! Every document is fetched at most once per loader, and both versions of
//! an API share one loader, so they see the same remote content.

pub mod address;
mod https;
mod policy;

use std::collections::HashMap;
use std::sync::Mutex;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loaded, Loader};
use faultline_source::{Limits, SourceName, decode_text};

pub use https::HttpsTransport;
pub use policy::{HostPattern, NetworkPolicy, Target};

/// What one HTTP GET produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// A successful response's body.
    Body(Vec<u8>),
    /// A redirect to the `Location` header's value.
    Redirect(String),
    /// Any other status.
    Status(u16),
}

/// Why a fetch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The policy forbids the request; the message says why.
    Refused(String),
    /// The body exceeded the byte limit.
    TooLarge { limit: u64 },
    /// Connection, TLS, timeout or protocol failure.
    Failed(String),
}

/// Performs single HTTP GET requests, without following redirects.
pub trait Transport: Send + Sync {
    /// Fetches `url`, reading at most `max_bytes` of body.
    fn get(&self, url: &str, max_bytes: u64) -> Result<Fetched, FetchError>;
}

/// A [`Loader`] for `https` documents that enforces a [`NetworkPolicy`].
pub struct RemoteLoader<T> {
    policy: NetworkPolicy,
    limits: Limits,
    transport: T,
    cache: Mutex<HashMap<String, Result<Loaded, LoadError>>>,
}

impl<T> std::fmt::Debug for RemoteLoader<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLoader")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl RemoteLoader<HttpsTransport> {
    /// A loader that fetches over HTTPS.
    pub fn https(policy: NetworkPolicy, limits: Limits) -> Self {
        let transport = HttpsTransport::new(&policy);
        Self::new(policy, limits, transport)
    }
}

impl<T: Transport> RemoteLoader<T> {
    pub fn new(policy: NetworkPolicy, limits: Limits, transport: T) -> Self {
        Self {
            policy,
            limits,
            transport,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn policy(&self) -> &NetworkPolicy {
        &self.policy
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn fetch(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        let max_bytes = self.policy.max_bytes.min(self.limits.max_file_bytes);
        let mut url = uri.clone();
        for _ in 0..=self.policy.max_redirects {
            self.policy.check(&url).map_err(LoadError::Refused)?;
            let text = url.to_string();
            match self.transport.get(&text, max_bytes) {
                Ok(Fetched::Body(bytes)) => {
                    self.limits
                        .check_file_size(bytes.len() as u64)
                        .map_err(LoadError::TooLarge)?;
                    let decoded = decode_text(&bytes).map_err(LoadError::Decode)?;
                    return Ok(Loaded {
                        name: SourceName::Url(uri.to_string()),
                        text: decoded.to_owned(),
                    });
                }
                Ok(Fetched::Redirect(location)) => {
                    let next = UriRef::parse(&location).map_err(|e| {
                        LoadError::Network(format!(
                            "{text} redirected to an invalid location {location:?}: {e}"
                        ))
                    })?;
                    url = url.resolve(&next).without_fragment().normalized();
                }
                Ok(Fetched::Status(404 | 410)) => return Err(LoadError::NotFound),
                Ok(Fetched::Status(status)) => {
                    return Err(LoadError::Network(format!(
                        "fetching {text} failed with HTTP status {status}"
                    )));
                }
                Err(FetchError::Refused(reason)) => return Err(LoadError::Refused(reason)),
                Err(FetchError::TooLarge { limit }) => {
                    return Err(LoadError::Network(format!(
                        "{text} is larger than the {limit}-byte limit for remote documents"
                    )));
                }
                Err(FetchError::Failed(message)) => {
                    return Err(LoadError::Network(format!(
                        "cannot fetch {text}: {message}"
                    )));
                }
            }
        }
        Err(LoadError::Network(format!(
            "{uri} redirected more than {} times",
            self.policy.max_redirects
        )))
    }
}

impl<T: Transport> Loader for RemoteLoader<T> {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        let uri = uri.without_fragment().normalized();
        let key = uri.to_string();
        if let Some(cached) = self.cache.lock().expect("cache lock").get(&key) {
            return cached.clone();
        }
        // Not holding the lock while fetching: another thread may fetch the
        // same document concurrently, which costs a request but not
        // correctness, since both results are equivalent.
        let result = self.fetch(&uri);
        self.cache
            .lock()
            .expect("cache lock")
            .entry(key)
            .or_insert(result)
            .clone()
    }
}
