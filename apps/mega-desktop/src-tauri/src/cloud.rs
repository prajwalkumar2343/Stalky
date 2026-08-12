use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth;

const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProfile {
    pub user_id: String,
    pub role: String,
    pub aal: Option<String>,
    pub session_id: Option<String>,
}

pub fn profile() -> Result<CloudProfile, String> {
    let backend_url =
        backend_url().ok_or_else(|| "Stalky Cloud is not configured for this build.".to_owned())?;
    let access_token = auth::access_token()?;
    let response = Client::builder()
        .timeout(NETWORK_TIMEOUT)
        .build()
        .map_err(|error| format!("could not prepare the cloud client: {error}"))?
        .get(format!("{backend_url}/v1/me"))
        .bearer_auth(access_token)
        .send()
        .map_err(|error| format!("Stalky Cloud request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Stalky Cloud rejected the request ({})",
            response.status()
        ));
    }
    response
        .json::<CloudProfile>()
        .map_err(|error| format!("Stalky Cloud returned an invalid profile: {error}"))
}

fn backend_url() -> Option<String> {
    let raw = std::env::var("STALKY_BACKEND_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("STALKY_BACKEND_URL").map(str::to_owned))?;
    validate_backend_url(&raw).then(|| raw.trim_end_matches('/').to_owned())
}

fn validate_backend_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    let is_local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    url.scheme() == "https" || (url.scheme() == "http" && is_local)
}

#[cfg(test)]
mod tests {
    use super::validate_backend_url;

    #[test]
    fn cloud_api_requires_tls_outside_loopback() {
        assert!(validate_backend_url("https://api.stalky.app"));
        assert!(validate_backend_url("http://127.0.0.1:8080"));
        assert!(!validate_backend_url("http://api.stalky.app"));
        assert!(!validate_backend_url("not-a-url"));
    }
}
