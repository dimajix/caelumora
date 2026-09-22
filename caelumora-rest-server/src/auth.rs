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
use axum::{Json, Router};
use caelumora_model::actor::ActorId;
use caelumora_model::config::AuthConfig;
use caelumora_model::security::SecurityContext;
use caelumora_model::user::UserId;
use caelumora_model::user_provider::{AuthError, Credential, UserManager};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Name of the cookie holding the session token.
pub const SESSION_COOKIE: &str = "caelumora_session";

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
    kid: String,
    lifetime: i64,
    max_session: i64,
    secure_cookie: bool,
}

impl TokenService {
    /// Creates the service from the signing key and the `auth` configuration.
    ///
    /// # Errors
    /// If the key is shorter than 32 bytes.
    pub fn new(key: &[u8], config: &AuthConfig) -> anyhow::Result<Self> {
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
            kid: config.jwt_secret.clone(),
            lifetime: config.token_lifetime_secs as i64,
            max_session: config.max_session_secs as i64,
            secure_cookie: config.secure_cookie,
        })
    }

    fn issue(&self, user: UserId, auth_time: i64, now: i64) -> anyhow::Result<String> {
        let claims = Claims {
            sub: user.0,
            iat: now,
            exp: (now + self.lifetime).min(auth_time + self.max_session),
            auth_time,
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(self.kid.clone());
        Ok(jsonwebtoken::encode(&header, &claims, &self.encoding)?)
    }

    /// Returns the claims of a valid, unexpired token signed with our key.
    fn verify(&self, token: &str) -> Option<Claims> {
        match jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation) {
            Ok(data) if data.header.kid.as_deref() == Some(self.kid.as_str()) => Some(data.claims),
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

    fn cookie(&self, token: &str, max_age: i64) -> HeaderValue {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        let value = format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{secure}");
        HeaderValue::from_str(&value).expect("a JWT is a valid header value")
    }

    fn session_cookie(&self, user: UserId, auth_time: i64, now: i64) -> anyhow::Result<HeaderValue> {
        let token = self.issue(user, auth_time, now)?;
        let max_age = (now + self.lifetime).min(auth_time + self.max_session) - now;
        Ok(self.cookie(&token, max_age))
    }

    fn clear_cookie(&self) -> HeaderValue {
        self.cookie("", 0)
    }
}

/// What the authentication layer needs: the token service and the chain of
/// user providers.
pub struct Auth {
    pub tokens: TokenService,
    pub users: Arc<UserManager>,
}

/// The authenticated caller, put into the request by `require_auth`. Taking it
/// as a handler argument makes the handler reject unauthenticated requests.
#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedUser {
    pub id: UserId,
}

impl AuthenticatedUser {
    /// The security context for actions taken on behalf of this user.
    pub fn security_context(&self) -> SecurityContext {
        SecurityContext::new(ActorId::User(self.id), Default::default(), Default::default())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<AuthenticatedUser>().copied().ok_or(StatusCode::UNAUTHORIZED)
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

fn unauthorized(auth: &Auth, had_cookie: bool) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    if had_cookie {
        response.headers_mut().insert(SET_COOKIE, auth.tokens.clear_cookie());
    }
    response
}

/// Middleware for every route that needs a logged-in user: verifies the
/// session cookie, renews the token when due, and puts `AuthenticatedUser`
/// into the request. Answers `401` without a valid token.
pub async fn require_auth(State(auth): State<Arc<Auth>>, mut request: Request, next: Next) -> Response {
    let token = session_token(request.headers());
    let Some(claims) = token.as_deref().and_then(|t| auth.tokens.verify(t)) else {
        return unauthorized(&auth, token.is_some());
    };
    let user = UserId(claims.sub);
    let now = Utc::now().timestamp();

    let renewed = if auth.tokens.needs_renewal(&claims, now) {
        match auth.users.get_user(user).await {
            Ok(Some(_)) => match auth.tokens.session_cookie(user, claims.auth_time, now) {
                Ok(cookie) => Some(cookie),
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
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }
    } else {
        None
    };

    tracing::Span::current().record("user_id", tracing::field::display(user));
    request.extensions_mut().insert(AuthenticatedUser { id: user });
    let mut response = next.run(request).await;
    if let Some(cookie) = renewed {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: UserId,
    pub username: String,
}

async fn login(State(auth): State<Arc<Auth>>, Json(request): Json<LoginRequest>) -> Response {
    let credential = Credential::Password(&request.password);
    match auth.users.authenticate(&request.username, &credential).await {
        Ok(user) => {
            let now = Utc::now().timestamp();
            match auth.tokens.session_cookie(user.id, now, now) {
                Ok(cookie) => {
                    let body = Json(UserResponse { id: user.id, username: user.username });
                    ([(SET_COOKIE, cookie)], body).into_response()
                }
                Err(e) => {
                    tracing::error!("Cannot issue session token: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(AuthError::InvalidCredentials) => StatusCode::UNAUTHORIZED.into_response(),
        Err(AuthError::Backend(_)) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Deletes the cookie. The token itself stays valid until it expires - see
/// the module documentation.
async fn logout(State(auth): State<Arc<Auth>>) -> Response {
    (StatusCode::NO_CONTENT, [(SET_COOKIE, auth.tokens.clear_cookie())]).into_response()
}

async fn me(State(auth): State<Arc<Auth>>, user: AuthenticatedUser) -> Result<Json<UserResponse>, StatusCode> {
    match auth.users.get_user(user.id).await {
        Ok(Some(u)) => Ok(Json(UserResponse { id: u.id, username: u.username })),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// The `/auth` routes: `login` and `logout` are public, `me` requires a
/// session.
pub fn router(auth: Arc<Auth>) -> Router {
    let protected = Router::new()
        .route("/auth/me", get(me))
        .route_layer(axum::middleware::from_fn_with_state(auth.clone(), require_auth));
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
    use caelumora_model::store::EntityStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user_provider::UserProvider;
    use caelumora_model::user_provider_impl::accept_any::AcceptAnyUserProvider;
    use tower::ServiceExt;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn config() -> AuthConfig {
        AuthConfig { token_lifetime_secs: 600, max_session_secs: 3600, ..AuthConfig::default() }
    }

    fn tokens() -> TokenService {
        TokenService::new(KEY, &config()).unwrap()
    }

    fn auth(providers: Vec<Arc<dyn UserProvider>>) -> Arc<Auth> {
        Arc::new(Auth { tokens: tokens(), users: Arc::new(UserManager::new(providers)) })
    }

    fn accept_any() -> Vec<Arc<dyn UserProvider>> {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        vec![Arc::new(AcceptAnyUserProvider::new(store))]
    }

    fn cookie_value(response: &Response) -> Option<String> {
        response.headers().get_all(SET_COOKIE).iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| v.strip_prefix(&format!("{SESSION_COOKIE}=")).map(|rest| rest.split(';').next().unwrap_or("").to_string()))
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
    fn session_cookie_is_http_only_strict_and_secure() {
        let cookie = tokens().cookie("abc", 60);
        let cookie = cookie.to_str().unwrap();
        for attribute in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/", "Max-Age=60"] {
            assert!(cookie.contains(attribute), "missing {attribute} in {cookie}");
        }
    }

    #[tokio::test]
    async fn login_sets_a_cookie_that_me_accepts() {
        let app = router(auth(accept_any()));

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
        let app = router(auth(accept_any()));

        let response = app.clone().oneshot(Request::get("/auth/me").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app.oneshot(get_with_cookie("/auth/me", "not-a-jwt")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(cookie_value(&response).as_deref(), Some(""), "an invalid cookie is cleared");
    }

    #[tokio::test]
    async fn login_fails_without_providers() {
        let app = router(auth(vec![]));
        let response = app.oneshot(post_json("/auth/login", r#"{"username":"alice","password":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(cookie_value(&response).is_none());
    }

    #[tokio::test]
    async fn renewal_issues_a_new_cookie_for_known_users_only() {
        let providers = accept_any();
        let auth = auth(providers.clone());
        let alice = providers[0].authenticate("alice", &Credential::Password("x")).await.unwrap().unwrap();
        let app = router(auth.clone());
        let old = Utc::now().timestamp() - 400;

        let token = auth.tokens.issue(alice.id, old, old).unwrap();
        let response = app.clone().oneshot(get_with_cookie("/auth/me", &token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let renewed = cookie_value(&response).expect("a token past half its lifetime is renewed");
        let claims = auth.tokens.verify(&renewed).unwrap();
        assert_eq!(claims.auth_time, old, "renewal keeps auth_time");
        assert!(claims.iat > old);

        let stranger = auth.tokens.issue(UserId::new(), old, old).unwrap();
        let response = app.oneshot(get_with_cookie("/auth/me", &stranger)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "renewal checks the provider");
    }

    #[tokio::test]
    async fn logout_clears_the_cookie() {
        let response = router(auth(accept_any())).oneshot(Request::post("/auth/logout").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(cookie_value(&response).as_deref(), Some(""));
    }
}
