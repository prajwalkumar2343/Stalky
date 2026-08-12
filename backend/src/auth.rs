use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;

use crate::error::{AppError, REQUEST_ID_HEADER};

const JWKS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const JWKS_STALE_IF_ERROR_TTL: Duration = Duration::from_secs(5 * 60);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const UNKNOWN_KID_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);
const JWKS_MAX_BODY_BYTES: usize = 64 * 1024;
const TOKEN_CLOCK_SKEW_SECONDS: u64 = 30;

type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Arc<JwkSet>, AuthError>> + Send + 'a>>;

trait JwksSource: Send + Sync {
    fn fetch(&self) -> FetchFuture<'_>;
}

#[derive(Clone)]
pub struct AuthState {
    issuer: Arc<str>,
    cache: Arc<JwksCache>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub user_id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    role: String,
    iat: u64,
    #[serde(default)]
    aal: Option<String>,
    session_id: String,
}

#[derive(Debug)]
enum AuthError {
    Credentials,
    Token,
    Claims,
    Jwks,
}

impl AuthError {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Credentials => "credentials",
            Self::Token => "token",
            Self::Claims => "claims",
            Self::Jwks => "jwks",
        }
    }
}

struct HttpJwksSource {
    client: reqwest::Client,
    endpoint: Url,
}

impl JwksSource for HttpJwksSource {
    fn fetch(&self) -> FetchFuture<'_> {
        Box::pin(async move {
            let mut response = self
                .client
                .get(self.endpoint.clone())
                .send()
                .await
                .map_err(|_| AuthError::Jwks)?
                .error_for_status()
                .map_err(|_| AuthError::Jwks)?;
            if response
                .content_length()
                .is_some_and(|length| length > JWKS_MAX_BODY_BYTES as u64)
            {
                return Err(AuthError::Jwks);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| AuthError::Jwks)? {
                if body.len().saturating_add(chunk.len()) > JWKS_MAX_BODY_BYTES {
                    return Err(AuthError::Jwks);
                }
                body.extend_from_slice(&chunk);
            }
            let keys = serde_json::from_slice::<JwkSet>(&body).map_err(|_| AuthError::Jwks)?;
            if keys.keys.is_empty() {
                return Err(AuthError::Jwks);
            }
            Ok(Arc::new(keys))
        })
    }
}

struct CachedKeys {
    keys: Arc<JwkSet>,
    expires_at: Instant,
    stale_until: Instant,
}

struct JwksCache {
    source: Arc<dyn JwksSource>,
    value: RwLock<Option<CachedKeys>>,
    refresh: Mutex<Option<Instant>>,
}

impl JwksCache {
    fn new(source: Arc<dyn JwksSource>) -> Self {
        Self {
            source,
            value: RwLock::new(None),
            refresh: Mutex::new(None),
        }
    }

    async fn key(&self, kid: &str) -> Result<Jwk, AuthError> {
        let cached = self.snapshot().await;
        if let Some(keys) = cached.as_ref().filter(|cached| cached.is_fresh()) {
            if let Some(key) = keys.keys.find(kid) {
                return Ok(key.clone());
            }
            return self
                .refresh(true)
                .await?
                .find(kid)
                .cloned()
                .ok_or(AuthError::Token);
        }

        let stale_key = cached
            .as_ref()
            .filter(|cached| cached.can_serve_stale())
            .and_then(|cached| cached.keys.find(kid))
            .cloned();
        match self.refresh(false).await {
            Ok(keys) => keys.find(kid).cloned().ok_or(AuthError::Token),
            Err(error) => stale_key.ok_or(error),
        }
    }

    async fn refresh(&self, forced: bool) -> Result<Arc<JwkSet>, AuthError> {
        let mut last_forced_refresh = self.refresh.lock().await;
        if let Some(cached) = self.snapshot().await.filter(CachedKeys::is_fresh)
            && (!forced
                || last_forced_refresh
                    .is_some_and(|last| last.elapsed() < UNKNOWN_KID_REFRESH_COOLDOWN))
        {
            return Ok(cached.keys);
        }

        let now = Instant::now();
        if forced {
            *last_forced_refresh = Some(now);
        }
        let keys = self.source.fetch().await?;
        *self.value.write().await = Some(CachedKeys {
            keys: keys.clone(),
            expires_at: now + JWKS_CACHE_TTL,
            stale_until: now + JWKS_CACHE_TTL + JWKS_STALE_IF_ERROR_TTL,
        });
        Ok(keys)
    }

    async fn snapshot(&self) -> Option<CachedKeys> {
        self.value.read().await.as_ref().cloned()
    }
}

impl Clone for CachedKeys {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            expires_at: self.expires_at,
            stale_until: self.stale_until,
        }
    }
}

impl CachedKeys {
    fn is_fresh(&self) -> bool {
        self.expires_at > Instant::now()
    }

    fn can_serve_stale(&self) -> bool {
        self.stale_until > Instant::now()
    }
}

impl AuthState {
    pub fn new(supabase_url: &Url) -> Result<Self, reqwest::Error> {
        let base = supabase_url.as_str().trim_end_matches('/');
        let endpoint = Url::parse(&format!("{base}/auth/v1/.well-known/jwks.json"))
            .expect("a validated Supabase URL must produce a valid JWKS URL");
        let client = reqwest::Client::builder()
            .timeout(JWKS_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stalky-backend/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self::with_source(
            format!("{base}/auth/v1"),
            Arc::new(HttpJwksSource { client, endpoint }),
        ))
    }

    fn with_source(issuer: String, source: Arc<dyn JwksSource>) -> Self {
        Self {
            issuer: issuer.into(),
            cache: Arc::new(JwksCache::new(source)),
        }
    }

    async fn verify(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let token = bearer_token(headers)?;
        let token_header = decode_header(token).map_err(|_| AuthError::Token)?;
        if !matches!(token_header.alg, Algorithm::ES256 | Algorithm::RS256) {
            return Err(AuthError::Token);
        }
        let kid = token_header.kid.as_deref().ok_or(AuthError::Token)?;

        let jwk = self.cache.key(kid).await?;
        if !jwk_allows_algorithm(&jwk, token_header.alg) {
            return Err(AuthError::Token);
        }
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| AuthError::Token)?;
        let mut validation = Validation::new(token_header.alg);
        validation.set_issuer(&[self.issuer.as_ref()]);
        validation.set_audience(&["authenticated"]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = TOKEN_CLOCK_SKEW_SECONDS;
        validation.validate_nbf = true;

        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| AuthError::Token)?
            .claims;
        Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Claims)?;
        if claims.role != "authenticated" {
            return Err(AuthError::Claims);
        }
        Uuid::parse_str(&claims.session_id).map_err(|_| AuthError::Claims)?;
        if claims.iat > now_seconds().saturating_add(TOKEN_CLOCK_SKEW_SECONDS) {
            return Err(AuthError::Claims);
        }
        Ok(Principal {
            user_id: claims.sub,
            role: claims.role,
            aal: claims.aal,
            session_id: Some(claims.session_id),
        })
    }
}

fn jwk_allows_algorithm(jwk: &Jwk, algorithm: Algorithm) -> bool {
    let algorithm_matches = matches!(
        (jwk.common.key_algorithm, algorithm),
        (None, _)
            | (Some(KeyAlgorithm::ES256), Algorithm::ES256)
            | (Some(KeyAlgorithm::RS256), Algorithm::RS256)
    );
    let signing_key = matches!(
        jwk.common.public_key_use,
        None | Some(PublicKeyUse::Signature)
    );
    let verifies = jwk
        .common
        .key_operations
        .as_ref()
        .is_none_or(|operations| operations.contains(&KeyOperations::Verify));
    algorithm_matches && signing_key && verifies
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthError::Credentials)?
        .to_str()
        .map_err(|_| AuthError::Credentials)?;
    let (scheme, token) = raw.split_once(' ').ok_or(AuthError::Credentials)?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(AuthError::Credentials);
    }
    Ok(token)
}

pub async fn require_auth(
    State(state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    match state.verify(request.headers()).await {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(error) => {
            tracing::debug!(reason = error.kind(), "authentication rejected");
            let request_id = request
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            AppError::unauthorized()
                .with_request_id(request_id)
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode, jwk::Jwk};
    use serde::Serialize;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    const ISSUER: &str = "https://project.supabase.co/auth/v1";
    const USER_ID: &str = "11111111-1111-4111-8111-111111111111";
    const SESSION_ID: &str = "22222222-2222-4222-8222-222222222222";
    const EC_PRIVATE_KEY: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgFJWFNUek3jBEQ1oD
13yibTbhhZMDn3Xfk/j425+4tM+hRANCAARviZUCfVIDRGMpA+fUoHkOcQ1RHHus
d7SqNsOoT1RCvHLTS6nlM8yF0Ri6VshPqqQTV81Ehkltex7dYbPLa/iD
-----END PRIVATE KEY-----"#;

    struct StaticSource(Arc<JwkSet>);

    impl JwksSource for StaticSource {
        fn fetch(&self) -> FetchFuture<'_> {
            let keys = self.0.clone();
            Box::pin(async move { Ok(keys) })
        }
    }

    struct SequenceSource {
        responses: StdMutex<VecDeque<Result<Arc<JwkSet>, AuthError>>>,
        calls: AtomicUsize,
    }

    impl SequenceSource {
        fn new(responses: Vec<Result<Arc<JwkSet>, AuthError>>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl JwksSource for SequenceSource {
        fn fetch(&self) -> FetchFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let response = self.responses.lock().unwrap().pop_front().unwrap();
            Box::pin(async move { response })
        }
    }

    #[derive(Clone, Serialize)]
    struct TestClaims<'a> {
        sub: &'a str,
        role: &'a str,
        aal: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<&'a str>,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
        iat: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        nbf: Option<u64>,
    }

    fn test_jwk(key: &EncodingKey, kid: &str) -> Jwk {
        let mut jwk = Jwk::from_encoding_key(key, Algorithm::ES256).unwrap();
        jwk.common.key_id = Some(kid.to_owned());
        jwk
    }

    fn state() -> (AuthState, EncodingKey) {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE_KEY).unwrap();
        let jwk = test_jwk(&key, "test-key");
        let source = Arc::new(StaticSource(Arc::new(JwkSet { keys: vec![jwk] })));
        (AuthState::with_source(ISSUER.to_owned(), source), key)
    }

    fn claims(role: &str) -> TestClaims<'_> {
        TestClaims {
            sub: USER_ID,
            role,
            aal: "aal1",
            session_id: Some(SESSION_ID),
            iss: ISSUER,
            aud: "authenticated",
            exp: now_seconds() + 3_600,
            iat: now_seconds(),
            nbf: None,
        }
    }

    fn token(key: &EncodingKey, claims: &TestClaims<'_>) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("test-key".to_owned());
        encode(&header, claims, key).unwrap()
    }

    fn authorization_header(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn accepts_valid_asymmetric_supabase_token() {
        let (state, key) = state();
        let headers = authorization_header(&token(&key, &claims("authenticated")));
        let principal = state.verify(&headers).await.unwrap();
        assert_eq!(principal.user_id, USER_ID);
        assert_eq!(principal.session_id.as_deref(), Some(SESSION_ID));
    }

    #[tokio::test]
    async fn rejects_symmetric_algorithm_before_key_lookup() {
        let (state, _) = state();
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-key".to_owned());
        let token = encode(
            &header,
            &claims("authenticated"),
            &EncodingKey::from_secret(b"not-used-by-server"),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert!(matches!(
            state.verify(&headers).await,
            Err(AuthError::Token)
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_role_claim() {
        let (state, key) = state();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", token(&key, &claims("anon")))
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            state.verify(&headers).await,
            Err(AuthError::Claims)
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_temporal_and_identity_claims() {
        let (state, key) = state();
        let now = now_seconds();
        let mut cases = Vec::new();

        let mut wrong_issuer = claims("authenticated");
        wrong_issuer.iss = "https://attacker.invalid/auth/v1";
        cases.push(wrong_issuer);

        let mut wrong_audience = claims("authenticated");
        wrong_audience.aud = "anon";
        cases.push(wrong_audience);

        let mut expired = claims("authenticated");
        expired.exp = now.saturating_sub(60);
        cases.push(expired);

        let mut future_nbf = claims("authenticated");
        future_nbf.nbf = Some(now + 300);
        cases.push(future_nbf);

        let mut future_iat = claims("authenticated");
        future_iat.iat = now + 300;
        cases.push(future_iat);

        let mut missing_session = claims("authenticated");
        missing_session.session_id = None;
        cases.push(missing_session);

        let mut invalid_session = claims("authenticated");
        invalid_session.session_id = Some("not-a-uuid");
        cases.push(invalid_session);

        for claims in cases {
            let headers = authorization_header(&token(&key, &claims));
            assert!(state.verify(&headers).await.is_err());
        }
    }

    #[tokio::test]
    async fn refreshes_once_when_a_token_uses_a_new_key_id() {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE_KEY).unwrap();
        let first = Arc::new(JwkSet {
            keys: vec![test_jwk(&key, "old-key")],
        });
        let second = Arc::new(JwkSet {
            keys: vec![test_jwk(&key, "new-key")],
        });
        let source = Arc::new(SequenceSource::new(vec![Ok(first), Ok(second)]));
        let state = AuthState::with_source(ISSUER.to_owned(), source.clone());
        let mut old_header = Header::new(Algorithm::ES256);
        old_header.kid = Some("old-key".to_owned());
        let old_token = encode(&old_header, &claims("authenticated"), &key).unwrap();
        state
            .verify(&authorization_header(&old_token))
            .await
            .unwrap();

        let mut new_header = Header::new(Algorithm::ES256);
        new_header.kid = Some("new-key".to_owned());
        let new_token = encode(&new_header, &claims("authenticated"), &key).unwrap();
        state
            .verify(&authorization_header(&new_token))
            .await
            .unwrap();
        assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_cold_cache_reads_share_one_jwks_fetch() {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE_KEY).unwrap();
        let keys = Arc::new(JwkSet {
            keys: vec![test_jwk(&key, "shared-key")],
        });
        let source = Arc::new(SequenceSource::new(vec![Ok(keys)]));
        let cache = JwksCache::new(source.clone());

        let (first, second) = tokio::join!(cache.key("shared-key"), cache.key("shared-key"));

        first.unwrap();
        second.unwrap();
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn serves_only_a_known_stale_key_when_refresh_fails() {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE_KEY).unwrap();
        let source = Arc::new(SequenceSource::new(vec![
            Err(AuthError::Jwks),
            Err(AuthError::Jwks),
        ]));
        let cache = JwksCache::new(source);
        let now = Instant::now();
        *cache.value.write().await = Some(CachedKeys {
            keys: Arc::new(JwkSet {
                keys: vec![test_jwk(&key, "known-key")],
            }),
            expires_at: now - Duration::from_secs(1),
            stale_until: now + Duration::from_secs(30),
        });

        assert_eq!(
            cache
                .key("known-key")
                .await
                .unwrap()
                .common
                .key_id
                .as_deref(),
            Some("known-key")
        );
        assert!(matches!(
            cache.key("unknown-key").await,
            Err(AuthError::Jwks)
        ));
    }

    #[tokio::test]
    async fn rejects_jwk_marked_for_encryption() {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE_KEY).unwrap();
        let mut jwk = test_jwk(&key, "test-key");
        jwk.common.public_key_use = Some(PublicKeyUse::Encryption);
        let state = AuthState::with_source(
            ISSUER.to_owned(),
            Arc::new(StaticSource(Arc::new(JwkSet { keys: vec![jwk] }))),
        );
        let headers = authorization_header(&token(&key, &claims("authenticated")));

        assert!(matches!(
            state.verify(&headers).await,
            Err(AuthError::Token)
        ));
    }

    #[test]
    fn bearer_parser_rejects_ambiguous_headers() {
        for value in [
            "Bearer",
            "Basic token",
            "Bearer  token",
            "Bearer token extra",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            assert!(bearer_token(&headers).is_err(), "accepted {value:?}");
        }
    }
}
