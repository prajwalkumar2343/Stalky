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
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("STALKY_GOOGLE_CLIENT_ID").map(str::to_owned))
}

pub fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256",
        encode(client_id),
        encode(redirect_uri),
        encode(GOOGLE_SCOPES),
        encode(state),
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

#[cfg(target_os = "macos")]
mod native {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use getrandom::fill;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::{GoogleIdentity, authorization_url, client_id};

    const KEYCHAIN_SERVICE: &str = "com.stalky.desktop.google";
    const KEYCHAIN_ACCOUNT: &str = "oauth-session";
    const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

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
        get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).is_ok()
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
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth2/callback");
        let state = random_urlsafe(32)?;
        let verifier = random_urlsafe(64)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let url = authorization_url(&client_id, &redirect_uri, &state, &challenge);
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("stalky-google-oauth-callback".to_owned())
            .spawn(move || wait_for_callback(listener, sender))
            .map_err(|error| format!("could not start the OAuth callback listener: {error}"))?;
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|error| format!("could not open the system browser: {error}"))?;
        let callback = receiver.recv_timeout(CALLBACK_TIMEOUT).map_err(|_| {
            "Google sign-in timed out. You can try again from Settings.".to_owned()
        })??;
        if callback.state != state {
            return Err("Google sign-in could not verify the browser response.".to_owned());
        }
        let session = exchange_code(&client_id, &callback.code, &verifier, &redirect_uri)?;
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
        sender: mpsc::SyncSender<Result<Callback, String>>,
    ) {
        let result = listener
            .accept()
            .map_err(|error| format!("OAuth callback failed: {error}"))
            .and_then(|(mut stream, _)| read_callback(&mut stream));
        let _ = sender.send(result);
    }

    fn read_callback(stream: &mut TcpStream) -> Result<Callback, String> {
        let mut buffer = [0_u8; 8192];
        let bytes = stream
            .read(&mut buffer)
            .map_err(|error| format!("could not read OAuth callback: {error}"))?;
        let request = String::from_utf8_lossy(&buffer[..bytes]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| "OAuth callback did not contain a request target.".to_owned())?;
        let query = target.split_once('?').map_or("", |(_, query)| query);
        let params = query.split('&').filter_map(|part| part.split_once('='));
        let mut code = None;
        let mut state = None;
        for (key, value) in params {
            match key {
                "code" => code = Some(decode(value)?),
                "state" => state = Some(decode(value)?),
                "error" => return Err(format!("Google sign-in was not completed ({value}).")),
                _ => {}
            }
        }
        let callback = Callback {
            code: code.ok_or_else(|| "Google did not return an authorization code.".to_owned())?,
            state: state.ok_or_else(|| "Google did not return an OAuth state.".to_owned())?,
        };
        write_response(stream, &callback)?;
        Ok(callback)
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
    }

    fn exchange_code(
        client_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<GoogleSession, String> {
        let response = reqwest::blocking::Client::new()
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
        Ok(GoogleSession {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_in: token.expires_in,
            token_type: token.token_type,
        })
    }

    fn fetch_identity(access_token: &str) -> Result<GoogleIdentity, String> {
        let response = reqwest::blocking::Client::new()
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
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert_eq!(GOOGLE_SCOPES, "openid email profile");
        assert!(!url.contains("client_secret"));
    }
}
