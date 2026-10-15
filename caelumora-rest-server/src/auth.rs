//! Authentication for the REST server (issue 10).
//!
//! A browser logs in once with username and password; the server answers with
//! a signed JWT in an `HttpOnly`, `SameSite=Strict` cookie. There is no
//! server-side session store: the token is the session. It cannot be revoked,
//! which is bounded instead of solved:
//!
//! - tokens are short-lived and renewed by `require_auth` once past half their
//!   lifetime - and only then is the `UserManager` asked whether the user
//!   still exists, so a disabled user is out after at most one lifetime;
//! - `auth_time` is carried over on renewal, so a login ends after
//!   `max_session_secs` however active the user is.
//!
//! A cookie rather than an `Authorization` header because `EventSource`, which
//! the web UI uses for server-sent events, cannot set headers, and because an
//! `HttpOnly` cookie is out of reach of injected scripts.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use crate::error::{ApiError, Json};
use caelumora_rest_api::auth::{LoginRequest, UserResponse};
use caelumora_model::config::AuthConfig;
use caelumora_model::secret::{SecretError, SecretManager};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StoreError;
use caelumora_model::user::UserId;
use caelumora_model::user_provider::{AuthError, Credential, UserManager};
use caelumora_runtime::controller::ControllerManager;
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use anyhow::Result;
use std::time::Duration;
use tracing::Span;

/// Name of the cookie holding the session token.
pub const SESSION_COOKIE: &str = "caelumora_session";

/// Name of the cookie holding the session's expiry as a unix timestamp.
///
/// Deliberately *not* `HttpOnly`: the web UI reads it to know when to renew
/// the session (issue 31). It carries no secret, only the time at which the
/// token in `SESSION_COOKIE` expires, and it is set and cleared together with
/// it - including by another tab, whose renewal this one then sees.
pub const SESSION_EXPIRES_COOKIE: &str = "caelumora_session_expires";

/// Signing keys shorter than this are refused: HS256 is only as strong as its
/// key, and a short one can be brute-forced offline from any captured token.
const MIN_KEY_LEN: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Claims {
    sub: Uuid,
    iat: i64,
    exp: i64,
    /// When the user entered their credentials; carried over on renewal.
    auth_time: i64,
}

/// Issues and verifies session tokens.
pub struct TokenService {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    /// The token's `kid`: the *name* of the secret that holds the signing
    /// key (`AuthConfig::jwt_secret`), never the key - so that pointing the
    /// configuration at another secret invalidates the old tokens.
    key_id: String,
    lifetime: i64,
    max_session: i64,
    secure_cookie: bool,
}

impl TokenService {
    /// Creates the service from the `auth` configuration, with the signing
    /// key resolved through `secrets` by the name it gives. If the secret
    /// does not exist, a random key is made - sessions then do not survive
    /// a restart, which is fine for development and loudly announced.
    ///
    /// # Errors
    /// What `secrets` reports other than a missing secret, and what `new`
    /// reports.
    pub async fn from_secret(secrets: &SecretManager, config: &AuthConfig) -> Result<Self> {
        let key = match secrets.resolve(&SecurityContext::system(), &config.jwt_secret).await {
            Ok(key) => key.into_bytes(),
            Err(SecretError::NotFound(_)) => {
                tracing::warn!("Secret '{}' not found: signing session tokens with a random key. \
                    Every restart logs all users out.", config.jwt_secret);
                let key: [u8; 32] = rand::random();
                key.to_vec()
            }
            Err(e) => return Err(e.into()),
        };
        Self::new(&key, config)
    }

    /// Creates the service from the signing key and the `auth` configuration.
    ///
    /// # Errors
    /// If the key is shorter than 32 bytes.
    pub fn new(key: &[u8], config: &AuthConfig) -> Result<Self> {
        if key.len() < MIN_KEY_LEN {
            anyhow::bail!("JWT signing key '{}' is too short: {} bytes, at least {} required", config.jwt_secret, key.len(), MIN_KEY_LEN);
        }
        // The algorithm is fixed here and never taken from a token's header.
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = 5;
        validation.set_required_spec_claims(&["exp", "sub"]);
        Ok(Self {
            encoding: EncodingKey::from_secret(key),
            decoding: DecodingKey::from_secret(key),
            validation,
            key_id: config.jwt_secret.clone(),
            lifetime: config.token_lifetime_secs as i64,
            max_session: config.max_session_secs as i64,
            secure_cookie: config.secure_cookie,
        })
    }

    /// When a token issued at `now` for a login made at `auth_time` expires:
    /// one lifetime later, or at the absolute limit of the login, whichever
    /// comes first.
    fn expires_at(&self, auth_time: i64, now: i64) -> i64 {
        (now + self.lifetime).min(auth_time + self.max_session)
    }

    fn issue(&self, user: UserId, auth_time: i64, now: i64) -> Result<String> {
        let claims = Claims {
            sub: user.0,
            iat: now,
            exp: self.expires_at(auth_time, now),
            auth_time,
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(self.key_id.clone());
        Ok(jsonwebtoken::encode(&header, &claims, &self.encoding)?)
    }

    /// Returns the claims of a valid, unexpired token signed with our key.
    fn verify(&self, token: &str) -> Option<Claims> {
        match jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation) {
            Ok(data) if data.header.kid.as_deref() == Some(self.key_id.as_str()) => Some(data.claims),
            Ok(_) => None,
            Err(e) => {
                tracing::debug!("Rejected session token: {}", e);
                None
            }
        }
    }

    /// A token is renewed once it is past half its lifetime, unless the
    /// absolute limit of the login has been reached.
    fn needs_renewal(&self, claims: &Claims, now: i64) -> bool {
        now - claims.iat >= self.lifetime / 2 && claims.auth_time + self.max_session > now
    }

    /// A `Set-Cookie` value. The values are a JWT (base64url and dots), a
    /// timestamp or nothing and the names constants, so this fails only if
    /// that stops being true.
    fn cookie(&self, name: &str, value: &str, max_age: i64, http_only: bool) -> Result<HeaderValue> {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        let http_only = if http_only { "; HttpOnly" } else { "" };
        let cookie = format!("{name}={value}{http_only}; SameSite=Strict; Path=/; Max-Age={max_age}{secure}");
        HeaderValue::from_str(&cookie).map_err(|_| anyhow::anyhow!("cookie {name} is no valid header value"))
    }

    /// The pair of cookies that carries a session - the token itself, and
    /// its expiry for the UI to read - and that expiry.
    fn session_cookies(&self, user: UserId, auth_time: i64, now: i64) -> Result<([HeaderValue; 2], i64)> {
        let token = self.issue(user, auth_time, now)?;
        let expires_at = self.expires_at(auth_time, now);
        let max_age = expires_at - now;
        let cookies = [
            self.cookie(SESSION_COOKIE, &token, max_age, true)?,
            self.cookie(SESSION_EXPIRES_COOKIE, &expires_at.to_string(), max_age, false)?,
        ];
        Ok((cookies, expires_at))
    }

    fn clear_cookies(&self) -> Result<[HeaderValue; 2]> {
        Ok([
            self.cookie(SESSION_COOKIE, "", 0, true)?,
            self.cookie(SESSION_EXPIRES_COOKIE, "", 0, false)?,
        ])
    }

    /// How long a freshly issued token is valid, in seconds.
    pub fn lifetime_secs(&self) -> u64 {
        self.lifetime.max(0) as u64
    }
}

/// What the authentication layer needs: the token service, the chain of
/// user providers, and the runtime's controllers, which give a user's
/// context with their roles as they are now (issue 304).
pub struct Auth {
    pub tokens: TokenService,
    pub users: Arc<UserManager>,
    pub controllers: Arc<ControllerManager>,
}

/// The authenticated caller, put into the request by `require_auth`. Taking it
/// as a handler argument makes the handler reject unauthenticated requests.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: UserId,
    /// The context the user acts in for this request, with their roles as
    /// their controller holds them when it began (issue 304).
    pub context: SecurityContext,
    /// When the session token that authenticated this request expires, as a
    /// unix timestamp - the one this request was renewed to, if it was. A
    /// long-running response outlives no request of its own to be checked by,
    /// so it has to end here by itself (issue 33).
    pub expires_at: i64,
}

impl AuthenticatedUser {
    /// The security context for actions taken on behalf of this user.
    pub fn security_context(&self) -> SecurityContext {
        self.context.clone()
    }

    /// How much of the session token is left, from now. Zero once it has
    /// expired.
    pub fn expires_in(&self) -> Duration {
        Duration::from_secs((self.expires_at - Utc::now().timestamp()).max(0) as u64)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<AuthenticatedUser>().cloned().ok_or_else(not_logged_in)
    }
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(COOKIE).iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

fn not_logged_in() -> ApiError {
    ApiError::unauthorized("not_authenticated", "Not logged in, or the session has expired")
}

fn unauthorized(auth: &Auth, had_cookie: bool) -> Response {
    let mut response = not_logged_in().into_response();
    if had_cookie {
        clear_session(auth, &mut response);
    }
    response
}

/// Lets `response` delete the session cookies.
fn clear_session(auth: &Auth, response: &mut Response) {
    match auth.tokens.clear_cookies() {
        Ok(cookies) => set_cookies(response, cookies),
        Err(e) => tracing::error!("Cannot clear the session cookies: {}", e),
    }
}

fn set_cookies(response: &mut Response, cookies: [HeaderValue; 2]) {
    for cookie in cookies {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
}

/// Middleware for every route that needs a logged-in user: verifies the
/// session cookie, renews the token when due, and puts `AuthenticatedUser`
/// into the request. Answers `401` without a valid token.
pub async fn require_auth(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    authenticate(auth, request, next, true).await
}

/// The same for the event streams, except that it never renews (issue 33).
///
/// A stream ends shortly before its token expires and its client reconnects,
/// so a renewing stream would be a session that renews itself for as long as
/// anything holds a stream open - a tab in the background, or a client that
/// is not a browser at all. Streams therefore consume a session rather than
/// extend it: what keeps one alive is [`refresh`], which the web UI calls
/// while its tab is visible (issue 31).
pub async fn require_stream_auth(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    authenticate(auth, request, next, false).await
}

async fn authenticate(auth: Arc<Auth>, mut request: Request, next: Next, renew: bool) -> Response {
    let token = session_token(request.headers());
    let Some(claims) = token.as_deref().and_then(|t| auth.tokens.verify(t)) else {
        return unauthorized(&auth, token.is_some());
    };
    let user = UserId(claims.sub);
    let now = Utc::now().timestamp();

    let mut expires_at = claims.exp;
    let renewed = if renew && auth.tokens.needs_renewal(&claims, now) {
        match auth.users.get_user(user).await {
            Ok(Some(_)) => match auth.tokens.session_cookies(user, claims.auth_time, now) {
                Ok((cookies, renewed_until)) => {
                    expires_at = renewed_until;
                    Some(cookies)
                }
                Err(e) => {
                    tracing::error!("Cannot renew session token for {}: {}", user, e);
                    None
                }
            },
            Ok(None) => {
                tracing::info!("Session of {} ended: the user is no longer known to any provider", user);
                return unauthorized(&auth, true);
            }
            Err(e) => {
                // Deny rather than extend a session nobody could vouch for.
                tracing::error!("Cannot check user {} on token renewal: {}", user, e);
                return ApiError::unavailable("user_provider_unavailable", "The session cannot be renewed: the user directory is not available").into_response();
            }
        }
    } else {
        None
    };

    // The user as this instance holds them, with their roles as they are
    // now (issues 304, 306).
    let context = match auth.controllers.user_controller(user).await {
        Ok(controller) => controller.security_context(Default::default(), Default::default()),
        Err(StoreError::NotFound { .. }) => {
            tracing::info!("Session of {} ended: the user no longer exists", user);
            return unauthorized(&auth, true);
        }
        Err(e) => {
            tracing::error!("Cannot read user {} for a request: {}", user, e);
            return ApiError::unavailable("user_unavailable", "The user cannot be read").into_response();
        }
    };
    Span::current().record("user_id", tracing::field::display(user));
    request.extensions_mut().insert(AuthenticatedUser { id: user, context, expires_at });
    let mut response = next.run(request).await;
    if let Some(cookies) = renewed {
        set_cookies(&mut response, cookies);
    }
    response
}

async fn login(State(auth): State<Arc<Auth>>, Json(request): Json<LoginRequest>) -> Response {
    let credential = Credential::Password(&request.password);
    match auth.users.authenticate(&request.username, &credential).await {
        Ok(user) => {
            let now = Utc::now().timestamp();
            match auth.tokens.session_cookies(user.id, now, now) {
                Ok((cookies, _)) => {
                    let body = Json(UserResponse {
                        id: user.id,
                        username: user.username.to_string(),
                        token_lifetime_secs: auth.tokens.lifetime_secs(),
                    });
                    let mut response = body.into_response();
                    set_cookies(&mut response, cookies);
                    response
                }
                Err(e) => {
                    ApiError::internal("session_token_failed", "issuing the session token", &e).into_response()
                }
            }
        }
        Err(AuthError::InvalidCredentials) => {
            ApiError::unauthorized("invalid_credentials", "Invalid username or password").into_response()
        }
        // The provider's own message may carry paths or hostnames: log only.
        Err(AuthError::Backend(_)) => {
            ApiError::unavailable("user_provider_unavailable", "Login is not possible: the user directory is not available").into_response()
        }
    }
}

/// Deletes the cookie. The token itself stays valid until it expires - see
/// the module documentation.
async fn logout(State(auth): State<Arc<Auth>>) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    clear_session(&auth, &mut response);
    response
}

/// Keeps a session alive while someone is watching a channel (issue 31).
///
/// The work is done by `require_auth`, which renews the token when it is past
/// half its lifetime - and only then asks the `UserManager` about the user.
/// This route exists so that clients have something to call for that and
/// nothing else: it needs no body, returns none, and is distinguishable from
/// `/auth/me` in the log.
async fn refresh(_user: AuthenticatedUser) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn me(State(auth): State<Arc<Auth>>, user: AuthenticatedUser) -> Response {
    match auth.users.get_user(user.id).await {
        Ok(Some(u)) => Json(UserResponse {
            id: u.id,
            username: u.username.to_string(),
            token_lifetime_secs: auth.tokens.lifetime_secs(),
        }).into_response(),
        // As when a renewal finds the user gone: the session ends, and so
        // does its cookie.
        Ok(None) => unauthorized(&auth, true),
        Err(_) => ApiError::unavailable("user_provider_unavailable", "The user directory is not available").into_response(),
    }
}

/// The `/auth` routes: `login` and `logout` are public, `me` requires a
/// session.
pub fn router(auth: Arc<Auth>) -> Router {
    let protected = Router::new()
        .route("/auth/me", get(me))
        .route("/auth/refresh", post(refresh))
        .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&auth), require_auth));
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .merge(protected)
        .with_state(auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_runtime::event::EventBus;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user_provider::UserProvider;
    use caelumora_model::user_provider::accept_any::AcceptAnyUserProvider;
    use tower::ServiceExt;
    use caelumora_model::confidentiality::ConfidentialityRules;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn config() -> AuthConfig {
        AuthConfig { token_lifetime_secs: 600, max_session_secs: 3600, ..AuthConfig::default() }
    }

    fn tokens() -> TokenService {
        TokenService::new(KEY, &config()).unwrap()
    }

    /// Auth asking `providers`, the users recorded in `store` (issue 306).
    fn auth(store: Arc<MemoryStore>, providers: Vec<Arc<dyn UserProvider>>) -> Arc<Auth> {
        let stores = store.entity_stores();
        let events = Arc::new(EventBus::new(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())));
        let controllers = Arc::new(ControllerManager::new(stores.clone(), Arc::<EventBus>::clone(&events), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()), Default::default()));
        Arc::new(Auth { tokens: tokens(), users: Arc::new(UserManager::new(providers, Arc::clone(&stores.users), events)), controllers })
    }

    /// Auth that accepts anybody, finding and recording users in one store.
    fn accepting_any() -> Arc<Auth> {
        let store = Arc::new(MemoryStore::new());
        let users = Arc::clone(&store.entity_stores().users);
        auth(store, vec![Arc::new(AcceptAnyUserProvider::new(users))])
    }

    fn set_cookie(response: &Response, name: &str) -> Option<String> {
        response.headers().get_all(SET_COOKIE).iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with(&format!("{name}=")))
            .map(|v| v.to_string())
    }

    fn cookie_value(response: &Response) -> Option<String> {
        set_cookie(response, SESSION_COOKIE)
            .map(|v| v[SESSION_COOKIE.len() + 1..].split(';').next().unwrap_or("").to_string())
    }

    fn post_json(uri: &str, body: &str) -> Request {
        Request::post(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()
    }

    fn get_with_cookie(uri: &str, token: &str) -> Request {
        Request::get(uri).header(COOKIE, format!("other=1; {SESSION_COOKIE}={token}")).body(Body::empty()).unwrap()
    }

    #[test]
    fn short_keys_are_refused() {
        assert!(TokenService::new(b"too short", &config()).is_err());
    }

    #[test]
    fn issued_tokens_verify_and_foreign_ones_do_not() {
        let service = tokens();
        let user = UserId::new();
        let now = Utc::now().timestamp();
        let token = service.issue(user, now, now).unwrap();

        let claims = service.verify(&token).expect("own token verifies");
        assert_eq!(claims.sub, user.0);
        assert_eq!(claims.exp, now + 600);

        let other = TokenService::new(b"another key, also 32 bytes long!", &config()).unwrap();
        assert!(other.verify(&token).is_none(), "a token signed with another key must be rejected");

        let rotated = TokenService::new(KEY, &AuthConfig { jwt_secret: "NEW_KEY".to_string(), ..config() }).unwrap();
        assert!(rotated.verify(&token).is_none(), "a token with another kid must be rejected");
    }

    #[test]
    fn expired_tokens_are_rejected() {
        let service = tokens();
        let past = Utc::now().timestamp() - 10_000;
        let token = service.issue(UserId::new(), past, past).unwrap();
        assert!(service.verify(&token).is_none());
    }

    #[test]
    fn expiry_never_exceeds_the_absolute_limit() {
        let service = tokens();
        let now = Utc::now().timestamp();
        let auth_time = now - 3500;
        let claims = service.verify(&service.issue(UserId::new(), auth_time, now).unwrap()).unwrap();
        assert_eq!(claims.exp, auth_time + 3600);
        assert!(!service.needs_renewal(&Claims { iat: now - 400, ..claims.clone() }, now + 200),
            "no renewal past the absolute limit");
    }

    #[test]
    fn renewal_is_due_after_half_the_lifetime() {
        let service = tokens();
        let now = Utc::now().timestamp();
        let claims = Claims { sub: Uuid::new_v4(), iat: now, exp: now + 600, auth_time: now };
        assert!(!service.needs_renewal(&claims, now + 299));
        assert!(service.needs_renewal(&claims, now + 300));
    }

    #[test]
    fn the_token_cookie_is_http_only_and_the_expiry_cookie_is_readable() {
        let now = Utc::now().timestamp();
        let ([token, expires], expires_at) = tokens().session_cookies(UserId::new(), now, now).unwrap();
        let token = token.to_str().unwrap();
        let expires = expires.to_str().unwrap();

        for attribute in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/", "Max-Age=600"] {
            assert!(token.contains(attribute), "missing {attribute} in {token}");
        }
        assert!(!expires.contains("HttpOnly"), "the UI must be able to read the expiry: {expires}");
        for attribute in ["SameSite=Strict", "Secure", "Path=/", "Max-Age=600"] {
            assert!(expires.contains(attribute), "missing {attribute} in {expires}");
        }
        let value: i64 = expires[SESSION_EXPIRES_COOKIE.len() + 1..].split(';').next().unwrap().parse().unwrap();
        assert_eq!(value, now + 600, "the expiry cookie holds when the token expires");
        assert_eq!(expires_at, value, "and says the same as the returned expiry");
    }

    #[tokio::test]
    async fn refresh_renews_a_session_that_is_past_half_its_lifetime() {
        let auth = accepting_any();
        let alice = auth.users.authenticate("alice", &Credential::Password("x")).await.unwrap();
        let app = router(Arc::clone(&auth));
        let post = |token: &str| Request::post("/auth/refresh")
            .header(COOKIE, format!("{SESSION_COOKIE}={token}"))
            .body(Body::empty())
            .unwrap();

        // Fresh token: nothing to do, and no new cookies.
        let now = Utc::now().timestamp();
        let fresh = auth.tokens.issue(alice.id, now, now).unwrap();
        let response = app.clone().oneshot(post(&fresh)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(cookie_value(&response).is_none(), "a fresh token is not renewed");

        // Past half its lifetime: renewed, both cookies set.
        let old = now - 400;
        let response = app.clone().oneshot(post(&auth.tokens.issue(alice.id, old, old).unwrap())).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let renewed = cookie_value(&response).expect("the token cookie is set again");
        assert_eq!(auth.tokens.verify(&renewed).unwrap().auth_time, old);
        assert!(set_cookie(&response, SESSION_EXPIRES_COOKIE).is_some(), "the expiry cookie is set again");

        // Without a session it is like any other protected route.
        let response = app.oneshot(Request::post("/auth/refresh").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_sets_a_cookie_that_me_accepts() {
        let app = router(accepting_any());

        let response = app.clone().oneshot(post_json("/auth/login", r#"{"username":"alice","password":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let token = cookie_value(&response).expect("login sets the session cookie");

        let response = app.oneshot(get_with_cookie("/auth/me", &token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("\"username\":\"alice\""));
    }

    #[tokio::test]
    async fn requests_without_a_valid_cookie_are_unauthorized() {
        let app = router(accepting_any());

        let response = app.clone().oneshot(Request::get("/auth/me").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app.oneshot(get_with_cookie("/auth/me", "not-a-jwt")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(cookie_value(&response).as_deref(), Some(""), "an invalid cookie is cleared");
    }

    #[tokio::test]
    async fn me_for_a_user_no_provider_knows_ends_the_session() {
        let auth = accepting_any();
        let now = Utc::now().timestamp();
        // A fresh token, so that no renewal asks about the user first.
        let token = auth.tokens.issue(UserId::new(), now, now).unwrap();

        let response = router(auth).oneshot(get_with_cookie("/auth/me", &token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(cookie_value(&response).as_deref(), Some(""), "the cookie of an ended session is cleared");
    }

    #[tokio::test]
    async fn login_fails_without_providers() {
        let app = router(auth(Arc::new(MemoryStore::new()), vec![]));
        let response = app.oneshot(post_json("/auth/login", r#"{"username":"alice","password":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(cookie_value(&response).is_none());
    }

    #[tokio::test]
    async fn renewal_issues_a_new_cookie_for_known_users_only() {
        let auth = accepting_any();
        let alice = auth.users.authenticate("alice", &Credential::Password("x")).await.unwrap();
        let app = router(Arc::clone(&auth));
        let old = Utc::now().timestamp() - 400;

        let token = auth.tokens.issue(alice.id, old, old).unwrap();
        let response = app.clone().oneshot(get_with_cookie("/auth/me", &token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let renewed = cookie_value(&response).expect("a token past half its lifetime is renewed");
        assert!(set_cookie(&response, SESSION_EXPIRES_COOKIE).is_some(), "the expiry cookie is renewed as well");
        let claims = auth.tokens.verify(&renewed).unwrap();
        assert_eq!(claims.auth_time, old, "renewal keeps auth_time");
        assert!(claims.iat > old);

        let stranger = auth.tokens.issue(UserId::new(), old, old).unwrap();
        let response = app.oneshot(get_with_cookie("/auth/me", &stranger)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "renewal checks the provider");
    }

    #[tokio::test]
    async fn a_renewed_request_carries_the_new_expiry_into_the_handler() {
        // What a stream ends at: the expiry of the token the request leaves
        // with, not of the one it arrived with (issue 33).
        let auth = accepting_any();
        let alice = auth.users.authenticate("alice", &Credential::Password("x")).await.unwrap();
        let app = Router::new()
            .route("/expires", get(|user: AuthenticatedUser| async move { user.expires_at.to_string() }))
            .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&auth), require_auth))
            .with_state(Arc::clone(&auth));

        let expiry_of = |response: Response| async move {
            let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
            String::from_utf8_lossy(&body).parse::<i64>().unwrap()
        };

        let now = Utc::now().timestamp();
        let fresh = auth.tokens.issue(alice.id, now, now).unwrap();
        let response = app.clone().oneshot(get_with_cookie("/expires", &fresh)).await.unwrap();
        assert_eq!(expiry_of(response).await, now + 600, "the expiry of the token as it arrived");

        let old = now - 400;
        let response = app.oneshot(get_with_cookie("/expires", &auth.tokens.issue(alice.id, old, old).unwrap())).await.unwrap();
        let expires_at = expiry_of(response).await;
        assert!(expires_at >= now + 599, "renewal moves the expiry along: {expires_at}");
    }

    #[tokio::test]
    async fn a_stream_request_consumes_the_session_instead_of_renewing_it() {
        // Otherwise anything holding a stream open would keep its own session
        // alive - including a tab nobody is looking at (issues 31, 33).
        let auth = accepting_any();
        let alice = auth.users.authenticate("alice", &Credential::Password("x")).await.unwrap();
        let app = Router::new()
            .route("/stream", get(|user: AuthenticatedUser| async move { user.expires_at.to_string() }))
            .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&auth), require_stream_auth))
            .with_state(Arc::clone(&auth));

        let old = Utc::now().timestamp() - 400;
        let response = app.oneshot(get_with_cookie("/stream", &auth.tokens.issue(alice.id, old, old).unwrap())).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(cookie_value(&response).is_none(), "a token past half its lifetime is not renewed here");
        let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body).parse::<i64>().unwrap(), old + 600,
            "the stream ends with the token it was opened with");
    }

    #[tokio::test]
    async fn logout_clears_the_cookie() {
        let response = router(accepting_any()).oneshot(Request::post("/auth/logout").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(cookie_value(&response).as_deref(), Some(""));
    }
}
