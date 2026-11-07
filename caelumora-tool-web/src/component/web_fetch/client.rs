//! How a page is fetched (issue 544): through the proxy the environment names, as the server
//! reaches the internet, and where the client connects itself - to an address written in the URL,
//! to a host the proxy is not asked for, on every redirect - never from an address inside the
//! network (`address::is_public`). Through the proxy, the proxy decides what it reaches (issue
//! 545). The proxies' own hosts are connected to wherever they lie, as the proxies they are, but no
//! page is fetched from them.
//!
//! A name is judged by every address it resolves to, by the resolver the client connects with, so
//! that what was judged is what is connected to: a name resolved again in between could have
//! changed its answer.

use super::address::is_public;
use super::text::{self, Page};
use crate::USER_AGENT;
use caelumora_api::error::describe_request_error;
use caelumora_domain::remote::redact_endpoint;
use encoding_rs::{Encoding, UTF_8};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::{Action, Attempt, Policy};
use reqwest::{Client as HttpClient, ClientBuilder, Error as ReqwestError, Url};
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net;
use url::Host;

/// How long a page may take, from connecting until its last byte.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How many redirects are followed.
const MAX_REDIRECTS: usize = 5;

/// How large a page may be, as it is sent, before it is read as text.
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

/// The variables a proxy is named by, as `reqwest` reads them.
const PROXY_VARIABLES: [&str; 6] =
    ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];

/// Why a page was not fetched. A URL is named without credentials, query and fragment.
#[derive(Debug, thiserror::Error)]
pub(super) enum FetchError {
    #[error("'{url}' is no URL: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error(transparent)]
    Refused(Refusal),
    #[error("'{url}' was answered with the status {status}")]
    Status { url: String, status: u16 },
    #[error("'{url}' is of the type '{media_type}', which cannot be read as text")]
    Unreadable { url: String, media_type: String },
    #[error("'{url}' names no type, so it cannot be read as text")]
    Untyped { url: String },
    #[error("'{url}' is larger than {} MiB", MAX_BODY_BYTES >> 20)]
    TooLarge { url: String },
    #[error("'{url}' could not be fetched: {reason}")]
    Failed { url: String, reason: String },
}

/// Why a URL is not followed, by the rules of what may be reached.
#[derive(Debug, Clone, thiserror::Error)]
pub(super) enum Refusal {
    #[error("only http and https URLs are fetched, not {0}")]
    Scheme(String),
    #[error("'{0}' lies inside the network, which is not fetched from")]
    InsideNetwork(String),
    #[error("'{0}' is the host of a proxy, which is not fetched from")]
    Proxy(String),
    #[error("the page was redirected more than {MAX_REDIRECTS} times")]
    Redirects,
}

impl FetchError {
    /// What failed in a request for `url`: a refusal of the rules where the resolver or a redirect
    /// gave one, else the chain of its causes, without the URL `reqwest` writes into it.
    fn of_request(url: &Url, error: ReqwestError) -> Self {
        let mut cause: Option<&(dyn Error + 'static)> = Some(&error);
        while let Some(current) = cause {
            if let Some(refusal) = current.downcast_ref::<Refusal>() {
                return FetchError::Refused(refusal.clone());
            }
            cause = current.source();
        }
        FetchError::Failed { url: shown(url), reason: describe_request_error(error) }
    }
}

/// `url` as a message names it.
fn shown(url: &Url) -> String {
    redact_endpoint(url.as_str())
}

/// What the client may reach: the addresses `admitted` - those of the public internet, but in a
/// test -, and the hosts of the proxies, which it connects to as proxies alone.
#[derive(Debug)]
struct Reach {
    admitted: fn(IpAddr) -> bool,
    proxies: HashSet<String>,
}

impl Reach {
    /// Whether `url` is followed, as far as the URL alone tells: one of http or https, and none
    /// whose host is a proxy's or an address not admitted. A name is judged as it is resolved
    /// (`PublicResolver`).
    ///
    /// # Errors
    /// The `Refusal` of what is not followed.
    fn follows(&self, url: &Url) -> Result<(), Refusal> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Refusal::Scheme(url.scheme().to_string()));
        }
        let address = match url.host() {
            Some(Host::Domain(name)) if self.proxies.contains(name) => {
                return Err(Refusal::Proxy(name.to_string()))
            }
            Some(Host::Ipv4(address)) => IpAddr::V4(address),
            Some(Host::Ipv6(address)) => IpAddr::V6(address),
            _ => return Ok(()),
        };
        if self.proxies.contains(&address.to_string()) {
            return Err(Refusal::Proxy(address.to_string()));
        }
        match (self.admitted)(address) {
            true => Ok(()),
            false => Err(Refusal::InsideNetwork(address.to_string())),
        }
    }
}

/// The client that fetches the pages of one component.
#[derive(Debug)]
pub(super) struct Client {
    http: HttpClient,
    reach: Arc<Reach>,
}

impl Client {
    /// A client that goes through the proxy the environment names and reaches the public
    /// internet.
    ///
    /// # Errors
    /// If no client can be made - its TLS cannot be set up.
    pub(super) fn new() -> reqwest::Result<Self> {
        let proxies = proxy_hosts(PROXY_VARIABLES.iter().filter_map(|name| env::var(name).ok()));
        Self::build(HttpClient::builder(), Reach { admitted: is_public, proxies })
    }

    /// A client that goes through no proxy and reaches the addresses `admitted`, with the proxies
    /// `proxies` - for the server of a test, which lies on the host itself.
    #[cfg(test)]
    fn direct(admitted: fn(IpAddr) -> bool, proxies: &[&str]) -> Self {
        let proxies = proxies.iter().map(|host| host.to_string()).collect();
        Self::build(HttpClient::builder().no_proxy(), Reach { admitted, proxies }).unwrap()
    }

    /// A client that goes through no proxy and reaches the public internet and the host itself -
    /// for the server of a test.
    #[cfg(test)]
    pub(super) fn reaching_the_host() -> Self {
        Self::direct(|address| is_public(address) || address.is_loopback(), &[])
    }

    fn build(builder: ClientBuilder, reach: Reach) -> reqwest::Result<Self> {
        let reach = Arc::new(reach);
        let followed = Arc::clone(&reach);
        let http = builder
            .dns_resolver(PublicResolver { reach: Arc::clone(&reach) })
            .redirect(Policy::custom(move |attempt| follow(&followed, attempt)))
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self { http, reach })
    }

    /// The page at `url`, read as text, decoded by the charset its type names - as UTF-8 where it
    /// names none.
    ///
    /// # Errors
    /// `InvalidUrl` for what is no URL, `Refused` for one the rules do not follow, `Status` for a
    /// page the server did not give, `Unreadable` or `Untyped` for one that cannot be read as
    /// text, `TooLarge` beyond `MAX_BODY_BYTES`, `Failed` where the server was not reached.
    pub(super) async fn fetch(&self, url: &str) -> Result<Page, FetchError> {
        let requested = Url::parse(url).map_err(|e| FetchError::InvalidUrl {
            url: redact_endpoint(url),
            reason: e.to_string(),
        })?;
        self.reach.follows(&requested).map_err(FetchError::Refused)?;
        let mut response = self
            .http
            .get(requested.clone())
            .send()
            .await
            .map_err(|e| FetchError::of_request(&requested, e))?;
        let url = response.url().clone();
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status { url: shown(&url), status: status.as_u16() });
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| FetchError::Untyped { url: shown(&url) })?;
        let mut parameters = content_type.split(';').map(str::trim);
        let media_type = parameters.next().unwrap_or_default().to_string();
        if !text::readable(&media_type) {
            return Err(FetchError::Unreadable { url: shown(&url), media_type });
        }
        let encoding = parameters
            .filter_map(|parameter| parameter.strip_prefix("charset="))
            .find_map(|charset| Encoding::for_label(charset.trim_matches('"').as_bytes()))
            .unwrap_or(UTF_8);
        let mut body = Vec::new();
        while let Some(chunk) =
            response.chunk().await.map_err(|e| FetchError::of_request(&url, e))?
        {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(FetchError::TooLarge { url: shown(&url) });
            }
            body.extend_from_slice(&chunk);
        }
        let (body, _, _) = encoding.decode(&body);
        Ok(text::page(url, &media_type, &body))
    }
}

/// Whether a redirect is followed: at most `MAX_REDIRECTS`, each as `reach` follows a URL.
fn follow(reach: &Reach, attempt: Attempt) -> Action {
    if attempt.previous().len() > MAX_REDIRECTS {
        return attempt.error(Refusal::Redirects);
    }
    match reach.follows(attempt.url()) {
        Ok(()) => attempt.follow(),
        Err(refusal) => attempt.error(refusal),
    }
}

/// The hosts of the proxies `proxies` name - URLs, or hosts with a port, as `reqwest` takes them.
fn proxy_hosts(proxies: impl Iterator<Item = String>) -> HashSet<String> {
    proxies
        .filter_map(|proxy| {
            Url::parse(&proxy).ok().filter(|url| url.has_host()).or_else(|| {
                // A proxy named without its scheme: `Url` would take its host for a scheme.
                Url::parse(&format!("http://{proxy}")).ok()
            })
        })
        .filter_map(|proxy| proxy.host_str().map(str::to_string))
        .collect()
}

/// The resolver the client connects with: it refuses a name with an address its reach does not
/// admit, but the names of the proxies - which no URL it follows names (`Reach::follows`), so
/// that they are resolved to connect to the proxy alone.
#[derive(Debug)]
struct PublicResolver {
    reach: Arc<Reach>,
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let proxy = self.reach.proxies.contains(&host);
        let admitted = self.reach.admitted;
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = net::lookup_host((host.as_str(), 0)).await?.collect();
            if !proxy && !addresses.iter().all(|address| admitted(address.ip())) {
                return Err(Refusal::InsideNetwork(host).into());
            }
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::RawQuery;
    use axum::http::header::LOCATION;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use tokio::net::TcpListener;

    /// A server of the test on the host itself, with a page of HTML, one in Latin-1, one of PNG,
    /// one of no type, one missing and one that redirects to `/redirect?to=...`; its port.
    async fn server() -> u16 {
        let router = Router::new()
            .route(
                "/page",
                get(|| async {
                    ([(CONTENT_TYPE, "text/html; charset=utf-8")], "<title>T</title><h1>Hi</h1>")
                }),
            )
            .route(
                "/latin",
                get(|| async {
                    ([(CONTENT_TYPE, "text/plain; charset=ISO-8859-1")], vec![0xfcu8])
                }),
            )
            .route("/image", get(|| async { ([(CONTENT_TYPE, "image/png")], vec![0u8; 4]) }))
            .route("/untyped", get(|| async { Response::new(Body::from("x")) }))
            .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/redirect",
                get(|query: RawQuery| async move {
                    let to = query.0.unwrap_or_default().trim_start_matches("to=").to_string();
                    (StatusCode::FOUND, [(LOCATION, to)]).into_response()
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    #[tokio::test]
    async fn a_page_is_fetched_and_read_as_text_by_its_charset() {
        let port = server().await;
        let client = Client::reaching_the_host();
        let page = client.fetch(&format!("http://localhost:{port}/page")).await.unwrap();
        assert_eq!(page.url.path(), "/page");
        assert_eq!(page.title.as_deref(), Some("T"));
        assert_eq!(page.text, "# Hi");
        let latin = client.fetch(&format!("http://localhost:{port}/latin")).await.unwrap();
        assert_eq!(latin.text, "ü", "decoded by its charset");
    }

    #[tokio::test]
    async fn a_page_the_server_does_not_give_or_that_is_no_text_fails_naming_its_url() {
        let port = server().await;
        let client = Client::reaching_the_host();
        let missing = client.fetch(&format!("http://u:secret@localhost:{port}/missing?k=s")).await;
        let Err(error @ FetchError::Status { status: 404, .. }) = missing else {
            panic!("{missing:?}")
        };
        let error = error.to_string();
        let url = format!("'http://localhost:{port}/missing'");
        assert!(error.contains(&url) && !error.contains("secret"), "{error}");
        let image = client.fetch(&format!("http://localhost:{port}/image")).await;
        assert!(
            matches!(&image, Err(FetchError::Unreadable { media_type, .. })
                if media_type == "image/png"),
            "{image:?}"
        );
        let untyped = client.fetch(&format!("http://localhost:{port}/untyped")).await;
        assert!(matches!(untyped, Err(FetchError::Untyped { .. })), "{untyped:?}");
    }

    /// The host itself is inside the network: neither by its name nor by its address, nor by a
    /// redirect from a page that may be fetched.
    #[tokio::test]
    async fn nothing_inside_the_network_is_fetched_by_name_address_or_redirect() {
        let port = server().await;
        let public = Client::direct(is_public, &[]);
        let by_name = public.fetch(&format!("http://localhost:{port}/page")).await;
        assert!(
            matches!(&by_name, Err(FetchError::Refused(Refusal::InsideNetwork(host)))
                if host == "localhost"),
            "{by_name:?}"
        );
        let by_address = public.fetch(&format!("http://127.0.0.1:{port}/page")).await;
        assert!(
            matches!(by_address, Err(FetchError::Refused(Refusal::InsideNetwork(_)))),
            "{by_address:?}"
        );
        let client = Client::reaching_the_host();
        let redirected = client
            .fetch(&format!("http://localhost:{port}/redirect?to=http://169.254.169.254/"))
            .await;
        assert!(
            matches!(redirected, Err(FetchError::Refused(Refusal::InsideNetwork(_)))),
            "{redirected:?}"
        );
        let followed = client
            .fetch(&format!("http://localhost:{port}/redirect?to=http://localhost:{port}/page"))
            .await;
        assert_eq!(followed.unwrap().url.path(), "/page", "a redirect within what may be reached");
    }

    /// A proxy's host is resolved to connect to the proxy, but no page is fetched from it -
    /// requested or redirected to -, wherever it lies.
    #[tokio::test]
    async fn no_page_is_fetched_from_a_proxys_host() {
        let port = server().await;
        let client = Client::direct(is_public, &["localhost", "127.0.0.1"]);
        for url in [format!("http://localhost:{port}/page"), format!("http://127.0.0.1:{port}/")] {
            let fetched = client.fetch(&url).await;
            assert!(matches!(fetched, Err(FetchError::Refused(Refusal::Proxy(_)))), "{fetched:?}");
        }
        let redirect =
            Client::direct(|address| is_public(address) || address.is_loopback(), &["proxy.test"]);
        let redirected = redirect
            .fetch(&format!("http://localhost:{port}/redirect?to=http://proxy.test/"))
            .await;
        assert!(
            matches!(redirected, Err(FetchError::Refused(Refusal::Proxy(_)))),
            "{redirected:?}"
        );
    }

    #[test]
    fn a_proxy_is_known_by_its_host_named_with_or_without_its_scheme() {
        let named = |proxy: &str| proxy_hosts([proxy.to_string()].into_iter());
        assert_eq!(
            named("http://proxy.ffm.example:8080"),
            HashSet::from(["proxy.ffm.example".to_string()])
        );
        assert_eq!(
            named("http://user:pw@proxy.example/"),
            HashSet::from(["proxy.example".to_string()])
        );
        assert_eq!(named("proxy.example:3128"), HashSet::from(["proxy.example".to_string()]));
        assert_eq!(named("socks5://10.0.0.1:1080"), HashSet::from(["10.0.0.1".to_string()]));
    }

    #[tokio::test]
    async fn what_is_no_http_url_is_not_fetched() {
        let client = Client::direct(is_public, &[]);
        let file = client.fetch("file:///etc/passwd").await;
        assert!(matches!(file, Err(FetchError::Refused(Refusal::Scheme(_)))), "{file:?}");
        let nothing = client.fetch("no url").await;
        assert!(matches!(nothing, Err(FetchError::InvalidUrl { .. })), "{nothing:?}");
    }
}
