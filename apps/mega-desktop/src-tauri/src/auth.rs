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
        configured: client_id().is_some(),
        signed_in: signed_in(),
        scopes: GOOGLE_SCOPES,
    }
}

pub fn client_id() -> Option<String> {
    std::env::var("STALKY_GOOGLE_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("STALKY_GOOGLE_CLIENT_ID").map(str::to_owned))
}

pub fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    nonce: &str,
) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256&nonce={}",
        encode(client_id),
        encode(redirect_uri),
        encode(GOOGLE_SCOPES),
        encode(state),
        encode(code_challenge),
        encode(nonce),
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

#[cfg(target_os = "macos")]
mod native {
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use getrandom::fill;
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode as decode_jwt, decode_header};
    use reqwest::blocking::{Client, Response};
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::{GoogleIdentity, authorization_url, client_id};

    const KEYCHAIN_SERVICE: &str = "com.stalky.desktop.google";
    const KEYCHAIN_ACCOUNT: &str = "oauth-session";
    const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
    const CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const CALLBACK_IO_TIMEOUT: Duration = Duration::from_secs(10);
    const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);
    const OPEN_BROWSER_TIMEOUT: Duration = Duration::from_secs(10);
    const CALLBACK_PATH: &str = "/oauth2/callback";

    #[derive(Clone, Debug, Deserialize, serde::Serialize)]
    struct GoogleSession {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
        token_type: Option<String>,
    }

    #[derive(Debug)]
    struct Callback {
        code: String,
        state: String,
    }

    pub fn has_session() -> bool {
        get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
            .ok()
            .and_then(|encoded| serde_json::from_slice::<GoogleSession>(&encoded).ok())
            .is_some_and(|session| !session.access_token.trim().is_empty())
    }

    pub fn sign_out() -> Result<(), String> {
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

    pub fn run() -> Result<GoogleIdentity, String> {
        let client_id = client_id()
            .ok_or_else(|| "Google sign-in is not configured for this build.".to_owned())?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("could not reserve a local OAuth callback: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("could not read the OAuth callback port: {error}"))?
            .port();
        let expected_host = format!("127.0.0.1:{port}");
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth2/callback");
        let state = random_urlsafe(32)?;
        let verifier = random_urlsafe(64)?;
        let nonce = random_urlsafe(32)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let url = authorization_url(&client_id, &redirect_uri, &state, &challenge, &nonce);
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not prepare the OAuth callback listener: {error}"))?;
        open_browser(&url)?;
        let callback = wait_for_callback(listener, &state, &expected_host)?;
        let session = exchange_code(&client_id, &callback.code, &verifier, &redirect_uri, &nonce)?;
        let identity = fetch_identity(&session.access_token)?;
        let encoded = serde_json::to_vec(&session).map_err(|error| error.to_string())?;
        set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &encoded)
            .map_err(|error| format!("could not store the Google session in Keychain: {error}"))?;
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

    #[derive(Debug, Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
        token_type: Option<String>,
        id_token: String,
    }

    #[derive(Debug, Deserialize)]
    struct IdTokenClaims {
        nonce: String,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleJwkSet {
        keys: Vec<GoogleJwk>,
    }

    #[derive(Debug, Deserialize)]
    struct GoogleJwk {
        kty: String,
        kid: String,
        n: String,
        e: String,
        alg: Option<String>,
    }

    fn exchange_code(
        client_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        nonce: &str,
    ) -> Result<GoogleSession, String> {
        let client = oauth_client()?;
        let response = client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("code", code),
                ("client_id", client_id),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
            ])
            .send()
            .map_err(|error| format!("Google token exchange failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Google token exchange was rejected ({})",
                response.status()
            ));
        }
        let token = response
            .json::<TokenResponse>()
            .map_err(|error| format!("Google returned an invalid token response: {error}"))?;
        if token.access_token.trim().is_empty() || token.id_token.trim().is_empty() {
            return Err("Google returned an incomplete token response.".to_owned());
        }
        validate_id_token(&client, &token.id_token, client_id, nonce)?;
        Ok(GoogleSession {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_in: token.expires_in,
            token_type: token.token_type,
        })
    }

    fn oauth_client() -> Result<Client, String> {
        Client::builder()
            .timeout(NETWORK_TIMEOUT)
            .build()
            .map_err(|error| format!("could not prepare the Google network client: {error}"))
    }

    fn validate_id_token(
        client: &Client,
        id_token: &str,
        client_id: &str,
        expected_nonce: &str,
    ) -> Result<(), String> {
        let header = decode_header(id_token)
            .map_err(|error| format!("Google returned an invalid identity token: {error}"))?;
        if header.alg != Algorithm::RS256 {
            return Err(
                "Google returned an identity token with an unsupported algorithm.".to_owned(),
            );
        }
        let key_id = header
            .kid
            .ok_or_else(|| "Google returned an identity token without a key id.".to_owned())?;
        let response = client
            .get("https://www.googleapis.com/oauth2/v3/certs")
            .send()
            .map_err(|error| format!("Google identity keys could not be loaded: {error}"))?;
        let keys = ensure_success(response, "Google identity keys")?
            .json::<GoogleJwkSet>()
            .map_err(|error| format!("Google returned invalid identity keys: {error}"))?;
        let jwk = keys
            .keys
            .into_iter()
            .find(|key| {
                key.kid == key_id
                    && key.kty == "RSA"
                    && key
                        .alg
                        .as_deref()
                        .is_none_or(|algorithm| algorithm == "RS256")
            })
            .ok_or_else(|| "Google returned no matching identity key.".to_owned())?;
        let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
            .map_err(|error| format!("Google identity key was invalid: {error}"))?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[client_id]);
        validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = decode_jwt::<IdTokenClaims>(id_token, &key, &validation)
            .map_err(|error| format!("Google identity token validation failed: {error}"))?
            .claims;
        if claims.nonce != expected_nonce {
            return Err(
                "Google identity token nonce did not match the sign-in request.".to_owned(),
            );
        }
        Ok(())
    }

    fn ensure_success(response: Response, service: &str) -> Result<Response, String> {
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!(
                "{service} request was rejected ({})",
                response.status()
            ))
        }
    }

    fn fetch_identity(access_token: &str) -> Result<GoogleIdentity, String> {
        let response = oauth_client()?
            .get("https://openidconnect.googleapis.com/v1/userinfo")
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("Google identity lookup failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Google identity lookup was rejected ({})",
                response.status()
            ));
        }
        response
            .json::<GoogleIdentity>()
            .map_err(|error| format!("Google returned an invalid identity response: {error}"))
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

#[cfg(test)]
mod tests {
    use super::{GOOGLE_SCOPES, authorization_url};

    #[test]
    fn authorization_url_uses_pkce_and_minimal_scopes() {
        let url = authorization_url(
            "client.apps.googleusercontent.com",
            "http://127.0.0.1:4321/oauth2/callback",
            "state-value",
            "challenge-value",
            "nonce-value",
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert!(url.contains("nonce=nonce-value"));
        assert_eq!(GOOGLE_SCOPES, "openid email profile");
        assert!(!url.contains("client_secret"));
    }
}
