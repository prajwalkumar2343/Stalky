use serde::{Deserialize, Serialize};

pub const GOOGLE_SCOPES: &str = "openid email profile";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAuthStatus {
    pub configured: bool,
    pub signed_in: bool,
    pub scopes: &'static str,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GoogleIdentity {
    pub email: Option<String>,
    pub name: Option<String>,
}

pub fn status() -> GoogleAuthStatus {
    GoogleAuthStatus {
        configured: supabase_configuration().is_some(),
        signed_in: signed_in(),
        scopes: GOOGLE_SCOPES,
    }
}

fn configured_value(name: &str, compiled: Option<&str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| compiled.map(str::to_owned))
}

pub fn supabase_configuration() -> Option<(String, String)> {
    let url = configured_value("STALKY_SUPABASE_URL", option_env!("STALKY_SUPABASE_URL"))?;
    let publishable_key = configured_value(
        "STALKY_SUPABASE_PUBLISHABLE_KEY",
        option_env!("STALKY_SUPABASE_PUBLISHABLE_KEY"),
    )?;
    if !valid_supabase_url(&url) {
        return None;
    }
    Some((url.trim_end_matches('/').to_owned(), publishable_key))
}

fn valid_supabase_url(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    let is_local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    let secure_transport = parsed.scheme() == "https" || (parsed.scheme() == "http" && is_local);
    secure_transport
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && matches!(parsed.path(), "" | "/")
}

pub fn authorization_url(supabase_url: &str, redirect_uri: &str, code_challenge: &str) -> String {
    format!(
        "{}/auth/v1/authorize?provider=google&redirect_to={}&scopes={}&code_challenge={}&code_challenge_method=s256",
        supabase_url.trim_end_matches('/'),
        encode(redirect_uri),
        encode(GOOGLE_SCOPES),
        encode(code_challenge),
    )
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            byte => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn session_is_present(access_token: &str, refresh_token: &str) -> bool {
    !access_token.trim().is_empty() && !refresh_token.trim().is_empty()
}

#[cfg(target_os = "macos")]
mod native {
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use getrandom::fill;
    use reqwest::blocking::Client;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::{GoogleIdentity, authorization_url, session_is_present, supabase_configuration};

    const KEYCHAIN_SERVICE: &str = "com.stalky.desktop.google";
    const KEYCHAIN_ACCOUNT: &str = "oauth-session";
    const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
    const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const CALLBACK_IO_TIMEOUT: Duration = Duration::from_secs(10);
    const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
    const OPEN_BROWSER_TIMEOUT: Duration = Duration::from_secs(10);
    const CALLBACK_PATH: &str = "/oauth2/callback";
    static SESSION_LIFECYCLE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    static SESSION_GENERATION: SessionGeneration = SessionGeneration::new();
    static LOGIN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

    #[derive(Deserialize, serde::Serialize)]
    struct SupabaseSession {
        access_token: String,
        refresh_token: String,
        expires_in: u64,
        #[serde(default)]
        expires_at: Option<u64>,
        token_type: String,
        user: SupabaseUser,
    }

    #[derive(Default, Deserialize, serde::Serialize)]
    struct SupabaseUser {
        email: Option<String>,
        #[serde(default)]
        user_metadata: UserMetadata,
    }

    #[derive(Default, Deserialize, serde::Serialize)]
    struct UserMetadata {
        full_name: Option<String>,
        name: Option<String>,
    }

    #[derive(Debug)]
    struct Callback {
        code: String,
        state: String,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RefreshFailureKind {
        Terminal,
        Transient,
    }

    struct RefreshError {
        kind: RefreshFailureKind,
    }

    struct LoginAttempt;

    struct SessionGeneration(AtomicU64);

    impl SessionGeneration {
        const fn new() -> Self {
            Self(AtomicU64::new(0))
        }

        fn snapshot(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }

        fn invalidate(&self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }

        fn is_current(&self, snapshot: u64) -> bool {
            self.snapshot() == snapshot
        }
    }

    impl LoginAttempt {
        fn begin() -> Result<Self, String> {
            LOGIN_IN_PROGRESS
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map(|_| Self)
                .map_err(|_| "Google sign-in is already in progress.".to_owned())
        }
    }

    impl Drop for LoginAttempt {
        fn drop(&mut self) {
            LOGIN_IN_PROGRESS.store(false, Ordering::Release);
        }
    }

    fn session_lifecycle_lock() -> Result<MutexGuard<'static, ()>, String> {
        SESSION_LIFECYCLE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| "The cloud session lock is unavailable. Restart Stalky.".to_owned())
    }

    pub fn has_session() -> bool {
        get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .ok()
            .and_then(|encoded| serde_json::from_slice::<SupabaseSession>(&encoded).ok())
            .is_some_and(|session| {
                session_is_present(&session.access_token, &session.refresh_token)
            })
    }

    pub fn sign_out() -> Result<(), String> {
        let remote_session = {
            let _guard = session_lifecycle_lock()?;
            SESSION_GENERATION.invalidate();
            let session = get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
                .ok()
                .and_then(|encoded| serde_json::from_slice::<SupabaseSession>(&encoded).ok());
            delete_local_session()?;
            supabase_configuration().zip(session)
        };
        if let Some(((supabase_url, publishable_key), session)) = remote_session
            && revoke_session(&supabase_url, &publishable_key, &session.access_token).is_err()
        {
            eprintln!("Supabase did not confirm remote logout; the local session was removed.");
        }
        Ok(())
    }

    fn delete_local_session() -> Result<(), String> {
        delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|error| error.to_string())
            .or_else(|error| {
                if error.contains("-25300") {
                    Ok(())
                } else {
                    Err(error)
                }
            })
    }

    pub fn access_token() -> Result<String, String> {
        let _guard = session_lifecycle_lock()?;
        let encoded = get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .map_err(|_| "Sign in with Google before connecting to Stalky Cloud.".to_owned())?;
        let mut session = serde_json::from_slice::<SupabaseSession>(&encoded)
            .map_err(|_| "The saved cloud session is invalid. Sign in again.".to_owned())?;
        if session
            .expires_at
            .is_none_or(|expires_at| expires_at <= now_seconds().saturating_add(60))
        {
            session = match refresh_session(&session.refresh_token) {
                Ok(session) => session,
                Err(error) if error.kind == RefreshFailureKind::Terminal => {
                    delete_local_session()?;
                    return Err("Your cloud session has ended. Sign in again.".to_owned());
                }
                Err(_) => {
                    return Err(
                        "Stalky could not refresh the cloud session. Try again shortly.".to_owned(),
                    );
                }
            };
            let encoded = serde_json::to_vec(&session).map_err(|error| error.to_string())?;
            set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &encoded).map_err(
                |error| format!("could not update the cloud session in Keychain: {error}"),
            )?;
        }
        Ok(session.access_token)
    }

    pub fn run() -> Result<GoogleIdentity, String> {
        let _login_attempt = LoginAttempt::begin()?;
        let session_generation = SESSION_GENERATION.snapshot();
        let (supabase_url, publishable_key) = supabase_configuration()
            .ok_or_else(|| "Supabase sign-in is not configured for this build.".to_owned())?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("could not reserve a local OAuth callback: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("could not read the OAuth callback port: {error}"))?
            .port();
        let expected_host = format!("127.0.0.1:{port}");
        let state = random_urlsafe(32)?;
        let redirect_uri = format!(
            "http://127.0.0.1:{port}/oauth2/callback?state={}",
            super::encode(&state)
        );
        let verifier = random_urlsafe(64)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let url = authorization_url(&supabase_url, &redirect_uri, &challenge);
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not prepare the OAuth callback listener: {error}"))?;
        open_browser(&url)?;
        let callback = wait_for_callback(listener, &state, &expected_host)?;
        let session = exchange_code(&supabase_url, &publishable_key, &callback.code, &verifier)?;
        let identity = GoogleIdentity {
            email: session.user.email.clone(),
            name: session
                .user
                .user_metadata
                .full_name
                .clone()
                .or_else(|| session.user.user_metadata.name.clone()),
        };
        let encoded = serde_json::to_vec(&session).map_err(|error| error.to_string())?;
        let guard = session_lifecycle_lock()?;
        let cancelled = !SESSION_GENERATION.is_current(session_generation);
        if !cancelled {
            set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &encoded).map_err(
                |error| format!("could not store the Google session in Keychain: {error}"),
            )?;
        }
        drop(guard);
        if cancelled {
            let _ = revoke_session(&supabase_url, &publishable_key, &session.access_token);
            return Err("Google sign-in was cancelled.".to_owned());
        }
        Ok(identity)
    }

    fn random_urlsafe(length: usize) -> Result<String, String> {
        let mut bytes = vec![0; length];
        fill(&mut bytes)
            .map_err(|error| format!("could not generate OAuth randomness: {error}"))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn wait_for_callback(
        listener: TcpListener,
        expected_state: &str,
        expected_host: &str,
    ) -> Result<Callback, String> {
        let deadline = Instant::now() + CALLBACK_TIMEOUT;
        loop {
            match listener.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        return Err("OAuth callback came from a non-loopback address.".to_owned());
                    }
                    stream
                        .set_read_timeout(Some(CALLBACK_IO_TIMEOUT))
                        .and_then(|()| stream.set_write_timeout(Some(CALLBACK_IO_TIMEOUT)))
                        .map_err(|error| {
                            format!("could not prepare the OAuth callback: {error}")
                        })?;
                    return read_callback(&mut stream, expected_state, expected_host);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(
                            "Google sign-in timed out. You can try again from Settings.".to_owned()
                        );
                    }
                    std::thread::sleep(CALLBACK_POLL_INTERVAL);
                }
                Err(error) => return Err(format!("OAuth callback failed: {error}")),
            }
        }
    }

    fn open_browser(url: &str) -> Result<(), String> {
        let mut child = std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|error| format!("could not open the system browser: {error}"))?;
        let deadline = Instant::now() + OPEN_BROWSER_TIMEOUT;
        loop {
            match child
                .try_wait()
                .map_err(|error| format!("could not inspect the system browser: {error}"))?
            {
                Some(status) if status.success() => return Ok(()),
                Some(status) => {
                    return Err(format!(
                        "could not open the system browser (exit status: {status})"
                    ));
                }
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("opening the system browser timed out.".to_owned());
                }
                None => std::thread::sleep(CALLBACK_POLL_INTERVAL),
            }
        }
    }

    fn read_callback(
        stream: &mut TcpStream,
        expected_state: &str,
        expected_host: &str,
    ) -> Result<Callback, String> {
        let mut buffer = [0_u8; 8192];
        let bytes = stream
            .read(&mut buffer)
            .map_err(|error| format!("could not read OAuth callback: {error}"))?;
        if bytes == buffer.len() {
            return Err("OAuth callback request was too large.".to_owned());
        }
        let request = String::from_utf8_lossy(&buffer[..bytes]);
        let mut request_parts = request
            .lines()
            .next()
            .ok_or_else(|| "OAuth callback did not contain a request line.".to_owned())?
            .split_whitespace();
        let method = request_parts.next();
        let target = request_parts.next();
        let version = request_parts.next();
        if method != Some("GET")
            || (version != Some("HTTP/1.1") && version != Some("HTTP/1.0"))
            || request_parts.next().is_some()
        {
            return Err("OAuth callback used an unsupported HTTP request.".to_owned());
        }
        let host_headers = request
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim())
            .collect::<Vec<_>>();
        if host_headers.len() != 1 || host_headers[0] != expected_host {
            return Err("OAuth callback used an unexpected local host.".to_owned());
        }
        let target =
            target.ok_or_else(|| "OAuth callback did not contain a request target.".to_owned())?;
        if !target.starts_with('/') {
            return Err("OAuth callback did not use a relative request target.".to_owned());
        }
        let parsed = url::Url::parse(&format!("http://localhost{target}"))
            .map_err(|_| "OAuth callback contained an invalid request target.".to_owned())?;
        if parsed.path() != CALLBACK_PATH || parsed.fragment().is_some() {
            return Err("OAuth callback used an unexpected redirect path.".to_owned());
        }
        let mut code = None;
        let mut state = None;
        for part in parsed.query().unwrap_or_default().split('&') {
            if part.is_empty() {
                continue;
            }
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| "OAuth callback contained a malformed query.".to_owned())?;
            match key {
                "code" if code.is_none() => code = Some(decode(value)?),
                "state" if state.is_none() => state = Some(decode(value)?),
                "error" => {
                    let _ = decode(value);
                    return Err("Google sign-in was not completed.".to_owned());
                }
                "code" | "state" => {
                    return Err("OAuth callback contained a duplicate parameter.".to_owned());
                }
                _ => {}
            }
        }
        let callback = Callback {
            code: code.ok_or_else(|| "Google did not return an authorization code.".to_owned())?,
            state: state.ok_or_else(|| "Google did not return an OAuth state.".to_owned())?,
        };
        if callback.state != expected_state {
            write_error_response(stream)?;
            return Err("Google sign-in could not verify the browser response.".to_owned());
        }
        write_response(stream, &callback)?;
        Ok(callback)
    }

    fn write_error_response(stream: &mut TcpStream) -> Result<(), String> {
        let body = "<html><body><h1>Stalky sign-in could not be verified</h1><p>You can close this tab and return to Stalky.</p></body></html>";
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| format!("could not complete OAuth callback: {error}"))
    }

    fn write_response(stream: &mut TcpStream, _callback: &Callback) -> Result<(), String> {
        let body = "<html><body><h1>Stalky sign-in complete</h1><p>You can return to Stalky.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| format!("could not complete OAuth callback: {error}"))
    }

    fn decode(value: &str) -> Result<String, String> {
        let mut output = Vec::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let hex = value
                    .get(index + 1..index + 3)
                    .ok_or_else(|| "OAuth callback contained malformed encoding.".to_owned())?;
                output.push(
                    u8::from_str_radix(hex, 16)
                        .map_err(|_| "OAuth callback contained malformed encoding.".to_owned())?,
                );
                index += 3;
            } else if bytes[index] == b'+' {
                output.push(b' ');
                index += 1;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(output).map_err(|_| "OAuth callback was not valid UTF-8.".to_owned())
    }

    fn exchange_code(
        supabase_url: &str,
        publishable_key: &str,
        code: &str,
        verifier: &str,
    ) -> Result<SupabaseSession, String> {
        let response = auth_client()?
            .post(format!("{supabase_url}/auth/v1/token?grant_type=pkce"))
            .header("apikey", publishable_key)
            .json(&serde_json::json!({
                "auth_code": code,
                "code_verifier": verifier,
            }))
            .send()
            .map_err(|error| format!("Supabase session exchange failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Supabase session exchange was rejected ({})",
                response.status()
            ));
        }
        let mut session = response
            .json::<SupabaseSession>()
            .map_err(|error| format!("Supabase returned an invalid session: {error}"))?;
        if !session_is_present(&session.access_token, &session.refresh_token)
            || !session.token_type.eq_ignore_ascii_case("bearer")
        {
            return Err("Supabase returned an incomplete session.".to_owned());
        }
        session.expires_at = Some(now_seconds().saturating_add(session.expires_in));
        Ok(session)
    }

    fn refresh_session(refresh_token: &str) -> Result<SupabaseSession, RefreshError> {
        let (supabase_url, publishable_key) = supabase_configuration().ok_or(RefreshError {
            kind: RefreshFailureKind::Terminal,
        })?;
        let response = auth_client()
            .map_err(|_| RefreshError {
                kind: RefreshFailureKind::Transient,
            })?
            .post(format!(
                "{supabase_url}/auth/v1/token?grant_type=refresh_token"
            ))
            .header("apikey", publishable_key)
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .map_err(|_| RefreshError {
                kind: RefreshFailureKind::Transient,
            })?;
        if !response.status().is_success() {
            return Err(RefreshError {
                kind: classify_refresh_status(response.status()),
            });
        }
        let mut session = response
            .json::<SupabaseSession>()
            .map_err(|_| RefreshError {
                kind: RefreshFailureKind::Transient,
            })?;
        if !session_is_present(&session.access_token, &session.refresh_token)
            || !session.token_type.eq_ignore_ascii_case("bearer")
        {
            return Err(RefreshError {
                kind: RefreshFailureKind::Transient,
            });
        }
        session.expires_at = Some(now_seconds().saturating_add(session.expires_in));
        Ok(session)
    }

    fn classify_refresh_status(status: reqwest::StatusCode) -> RefreshFailureKind {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            RefreshFailureKind::Transient
        } else {
            RefreshFailureKind::Terminal
        }
    }

    fn revoke_session(
        supabase_url: &str,
        publishable_key: &str,
        access_token: &str,
    ) -> Result<(), String> {
        auth_client()?
            .post(format!("{supabase_url}/auth/v1/logout?scope=local"))
            .header("apikey", publishable_key)
            .bearer_auth(access_token)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map(|_| ())
            .map_err(|_| "Supabase did not confirm remote logout.".to_owned())
    }

    fn now_seconds() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }

    fn auth_client() -> Result<Client, String> {
        Client::builder()
            .timeout(NETWORK_TIMEOUT)
            .build()
            .map_err(|error| format!("could not prepare the Supabase network client: {error}"))
    }

    #[cfg(test)]
    mod tests {
        use super::{RefreshFailureKind, SessionGeneration, classify_refresh_status};
        use reqwest::StatusCode;

        #[test]
        fn terminal_refresh_rejections_require_new_sign_in() {
            for status in [
                StatusCode::BAD_REQUEST,
                StatusCode::UNAUTHORIZED,
                StatusCode::FORBIDDEN,
            ] {
                assert_eq!(
                    classify_refresh_status(status),
                    RefreshFailureKind::Terminal
                );
            }
        }

        #[test]
        fn transient_refresh_failures_preserve_the_rotating_session() {
            for status in [
                StatusCode::TOO_MANY_REQUESTS,
                StatusCode::INTERNAL_SERVER_ERROR,
                StatusCode::SERVICE_UNAVAILABLE,
            ] {
                assert_eq!(
                    classify_refresh_status(status),
                    RefreshFailureKind::Transient
                );
            }
        }

        #[test]
        fn logout_invalidation_prevents_inflight_login_persistence() {
            let generation = SessionGeneration::new();
            let login_snapshot = generation.snapshot();

            generation.invalidate();

            assert!(!generation.is_current(login_snapshot));
        }
    }
}

pub fn signed_in() -> bool {
    #[cfg(target_os = "macos")]
    {
        native::has_session()
    }
    #[cfg(not(target_os = "macos"))]
    false
}

pub fn sign_out() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        native::sign_out()
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

pub fn run() -> Result<GoogleIdentity, String> {
    #[cfg(target_os = "macos")]
    {
        native::run()
    }
    #[cfg(not(target_os = "macos"))]
    Err("Google sign-in is only available in the macOS desktop app.".to_owned())
}

pub fn access_token() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        native::access_token()
    }
    #[cfg(not(target_os = "macos"))]
    Err("Stalky Cloud authentication is only available in the macOS desktop app.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{GOOGLE_SCOPES, authorization_url, session_is_present, valid_supabase_url};

    #[test]
    fn authorization_url_uses_pkce_and_minimal_scopes() {
        let url = authorization_url(
            "https://project.supabase.co",
            "http://127.0.0.1:4321/oauth2/callback",
            "challenge-value",
        );
        assert!(url.contains("/auth/v1/authorize?provider=google"));
        assert!(url.contains("code_challenge_method=s256"));
        assert!(url.contains("scopes=openid%20email%20profile"));
        assert_eq!(GOOGLE_SCOPES, "openid email profile");
        assert!(!url.contains("client_secret"));
    }

    #[test]
    fn supabase_session_requires_both_credentials() {
        assert!(session_is_present("access-token", "refresh-token"));
        assert!(!session_is_present("", "refresh-token"));
        assert!(!session_is_present("access-token", ""));
    }

    #[test]
    fn supabase_configuration_requires_a_secure_base_url() {
        assert!(valid_supabase_url("https://project.supabase.co"));
        assert!(valid_supabase_url("http://127.0.0.1:54321"));
        assert!(!valid_supabase_url("http://project.supabase.co"));
        assert!(!valid_supabase_url("https://project.supabase.co/other"));
        assert!(!valid_supabase_url("https://user@project.supabase.co"));
    }
}
