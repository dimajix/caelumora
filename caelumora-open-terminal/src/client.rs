//! The requests to one Open Terminal instance: its endpoint, its key and
//! how its answers and failures are told. What fails is told without the
//! endpoint and without anything the instance wrote - its messages name
//! paths inside it - and logged with the endpoint, shown without
//! credentials.

use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use caelumora_runtime::remote::describe_request_error;
use caelumora_runtime::terminal::{TerminalError, TerminalResult};
use reqwest::{Client as HttpClient, Method, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::warn;

/// How long a request waits for the instance, beyond the wait it asks for,
/// unless the endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The key an instance is reached with.
pub(crate) enum Key {
    /// The one the endpoint's `auth` names, resolved through the secret
    /// chain on every request - an instance the operator started.
    Endpoint(Arc<SecretManager>),
    /// One made for the instance when Caelumora started it.
    #[cfg(feature = "docker")]
    Made(String),
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(_) => f.write_str("Endpoint"),
            #[cfg(feature = "docker")]
            Self::Made(_) => f.write_str("Made(..)"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Client {
    endpoint: RemoteEndpoint,
    base: Url,
    key: Key,
    http: HttpClient,
    /// When a request was last sent.
    used: Mutex<Instant>,
}

impl Client {
    /// A client of the instance at `endpoint`, reached with `key` - through
    /// the proxy the environment names unless `direct`, as a container
    /// Caelumora started is reached.
    ///
    /// # Errors
    /// `Unavailable` for a URL that is none, or a client that cannot be
    /// made.
    pub(crate) fn new(endpoint: RemoteEndpoint, key: Key, direct: bool) -> TerminalResult<Self> {
        let base = Url::parse(&endpoint.url).map_err(|_| {
            TerminalError::Unavailable("the storage's endpoint is no URL".to_string())
        })?;
        if base.cannot_be_a_base() {
            return Err(TerminalError::Unavailable("the storage's endpoint is no URL".to_string()));
        }
        let builder = HttpClient::builder();
        let http = match direct {
            true => builder.no_proxy(),
            false => builder,
        }
        .build()
        .map_err(|e| {
            TerminalError::Unavailable(format!(
                "no client for Open Terminal: {}",
                describe_request_error(e)
            ))
        })?;
        Ok(Self { endpoint, base, key, http, used: Mutex::new(Instant::now()) })
    }

    /// How long no request was sent.
    #[cfg(feature = "docker")]
    pub(crate) fn idle_for(&self) -> Duration {
        self.used.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).elapsed()
    }

    /// The URL of `segments` below the endpoint, with `query`. A segment is
    /// encoded as one, whatever it holds.
    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }

    /// Sends `method` to `segments` with `query`, for a process of `scope`
    /// if one is named, built further by `build`; waits `wait` beyond the
    /// endpoint's timeout. Any status is an answer: what it means is the
    /// caller's to say.
    ///
    /// # Errors
    /// `Unavailable` where the instance was not reached, or refused the key.
    pub(crate) async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        scope: Option<&str>,
        wait: Duration,
        build: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> TerminalResult<Response> {
        *self.used.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
        let mut request = self
            .http
            .request(method, self.url(segments, query))
            .timeout(self.endpoint.timeout_or(DEFAULT_TIMEOUT) + wait);
        let key = match &self.key {
            // The storage is the operator's, configured as the server's
            // own keys are: its key is read in the system's name.
            Key::Endpoint(secrets) => self.endpoint.bearer_token(secrets, &SecurityContext::system()).await.map_err(|e| {
                warn!(endpoint = %self.endpoint.shown(), "The key of an Open Terminal storage could not be read: {e}");
                TerminalError::Unavailable("the storage's key could not be read".to_string())
            })?,
            #[cfg(feature = "docker")]
            Key::Made(key) => Some(key.clone()),
        };
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        if let Some(scope) = scope {
            request = request.header("X-Session-Id", scope);
        }
        let answer = build(request).send().await.map_err(|e| {
            let why = describe_request_error(e);
            warn!(endpoint = %self.endpoint.shown(), "Open Terminal was not reached: {why}");
            TerminalError::Unavailable(format!("Open Terminal was not reached: {why}"))
        })?;
        // 403 is a file the instance may not touch, not the key.
        if answer.status() == StatusCode::UNAUTHORIZED {
            warn!(endpoint = %self.endpoint.shown(), "Open Terminal refused the storage's key");
            return Err(TerminalError::Unavailable(
                "Open Terminal refused the storage's key".to_string(),
            ));
        }
        Ok(answer)
    }

    /// The body of `answer`, a success, as `T`.
    ///
    /// # Errors
    /// `Unavailable` for a status that is no success or a body that does
    /// not fit, naming what did not.
    pub(crate) async fn parsed<T: DeserializeOwned>(&self, answer: Response) -> TerminalResult<T> {
        let answer = self.success(answer)?;
        let body = answer.bytes().await.map_err(|e| {
            TerminalError::Unavailable(format!(
                "Open Terminal's answer broke off: {}",
                describe_request_error(e)
            ))
        })?;
        serde_json::from_slice(&body).map_err(|e| {
            warn!(endpoint = %self.endpoint.shown(), "Open Terminal answered in a form not understood: {e}");
            TerminalError::Unavailable(format!("Open Terminal answered in a form not understood: {e}"))
        })
    }

    /// `answer` if it is a success.
    ///
    /// # Errors
    /// `Unavailable`, naming the status - never what the instance wrote.
    pub(crate) fn success(&self, answer: Response) -> TerminalResult<Response> {
        match answer.status() {
            status if status.is_success() => Ok(answer),
            status => Err(self.refused(status)),
        }
    }

    /// That the instance answered `status` where it should not have.
    pub(crate) fn refused(&self, status: StatusCode) -> TerminalError {
        warn!(endpoint = %self.endpoint.shown(), %status, "Open Terminal refused a request");
        TerminalError::Unavailable(format!("Open Terminal answered {status}"))
    }
}
