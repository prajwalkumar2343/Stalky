use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{sync::Mutex, time::timeout};
use tokio_util::sync::CancellationToken;

use crate::{credentials::SecretString, protocol::AgentRunRequest};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct ProviderRequest {
    pub model: String,
    pub message: String,
    pub image_base64: Option<String>,
    pub image_mime_type: Option<String>,
}

impl ProviderRequest {
    pub fn from_run(input: &AgentRunRequest) -> Self {
        let context = json!({
            "memories": input.memories,
            "todos": input.todos,
            "apps": input.apps,
            "mini_apps": input.mini_apps,
            "automations": input.automations,
            "context_files": input.context_files,
        });
        let message = format!(
            "You are Aura. Return a compact JSON object with fields reply, emotion, created_emotion, and actions.\n\nUser request:\n{}\n\nLocal context:\n{}",
            input.message, context
        );
        Self {
            model: input.model.clone(),
            message,
            image_base64: input.image_base64.clone(),
            image_mime_type: input.image_mime_type.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProviderResponse {
    pub reply: String,
    pub emotion: String,
    pub created_emotion: Option<String>,
    pub actions: Vec<Value>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider credential is missing")]
    MissingCredential,
    #[error("provider is not configured")]
    NotConfigured,
    #[error("provider request was cancelled")]
    Cancelled,
    #[error("provider request timed out")]
    Timeout,
    #[error("provider returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("provider transport failed")]
    Transport,
    #[error("provider returned an invalid response")]
    InvalidResponse,
    #[error("provider is unsupported")]
    Unsupported,
}

impl ProviderError {
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::HttpStatus(408 | 425 | 429 | 500..=599) | Self::Transport
        )
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingCredential => "provider.missing_credential",
            Self::NotConfigured => "provider.not_configured",
            Self::Cancelled => "run.cancelled",
            Self::Timeout => "provider.timeout",
            Self::HttpStatus(_) => "provider.http_error",
            Self::Transport => "provider.transport_error",
            Self::InvalidResponse => "provider.invalid_response",
            Self::Unsupported => "provider.unsupported",
        }
    }
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    async fn complete(
        &self,
        request: ProviderRequest,
        credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError>;
}

#[derive(Clone)]
pub struct ProviderRegistry {
    providers: Arc<HashMap<String, Arc<dyn ProviderAdapter>>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        let client = Client::new();
        Self::from_adapters([
            Arc::new(GeminiAdapter::new(client.clone())) as Arc<dyn ProviderAdapter>,
            Arc::new(OpenAiAdapter::new(client.clone())) as Arc<dyn ProviderAdapter>,
            Arc::new(OpenRouterAdapter::new(client)) as Arc<dyn ProviderAdapter>,
        ])
    }
}

impl ProviderRegistry {
    pub fn from_adapters<const N: usize>(adapters: [Arc<dyn ProviderAdapter>; N]) -> Self {
        let providers = adapters
            .into_iter()
            .map(|adapter| (adapter.name().to_owned(), adapter))
            .collect();
        Self {
            providers: Arc::new(providers),
        }
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn ProviderAdapter>> {
        self.providers
            .get(&provider.trim().to_ascii_lowercase())
            .cloned()
    }

    pub async fn complete(
        &self,
        provider: &str,
        request: ProviderRequest,
        credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let adapter = self.get(provider).ok_or(ProviderError::Unsupported)?;
        adapter.complete(request, credential, cancellation).await
    }
}

#[derive(Clone)]
pub struct GeminiAdapter {
    client: Client,
    endpoint: String,
}

impl GeminiAdapter {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            endpoint: "https://generativelanguage.googleapis.com/v1beta/models".to_owned(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(client: Client, endpoint: &str) -> Self {
        Self {
            client,
            endpoint: endpoint.to_owned(),
        }
    }
}

#[async_trait]
impl ProviderAdapter for GeminiAdapter {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn complete(
        &self,
        request: ProviderRequest,
        credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let key = credential.as_str().trim();
        if key.is_empty() {
            return Err(ProviderError::MissingCredential);
        }
        let model = if request.model.trim().is_empty() {
            "gemini-2.5-flash"
        } else {
            request.model.trim()
        };
        let mut parts = vec![json!({ "text": request.message })];
        if let (Some(data), Some(mime)) = (
            request.image_base64.as_deref(),
            request.image_mime_type.as_deref(),
        ) {
            parts.push(
                json!({ "inlineData": { "mimeType": mime, "data": strip_data_prefix(data) } }),
            );
        }
        let url = format!(
            "{}/{model}:generateContent",
            self.endpoint.trim_end_matches('/')
        );
        let response = send_json(
            self.client
                .post(url)
                .header("x-goog-api-key", key)
                .json(&json!({ "contents": [{ "role": "user", "parts": parts }] })),
            cancellation,
        )
        .await?;
        let text = response
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find_map(|part| part.get("text").and_then(Value::as_str))
            })
            .ok_or(ProviderError::InvalidResponse)?;
        parse_assistant_response(text)
    }
}

#[derive(Clone)]
pub struct OpenAiAdapter {
    client: Client,
    endpoint: String,
}

impl OpenAiAdapter {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            endpoint: "https://api.openai.com/v1/responses".to_owned(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(client: Client, endpoint: &str) -> Self {
        Self {
            client,
            endpoint: endpoint.to_owned(),
        }
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiAdapter {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(
        &self,
        request: ProviderRequest,
        credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let key = credential.as_str().trim();
        if key.is_empty() {
            return Err(ProviderError::MissingCredential);
        }
        let model = if request.model.trim().is_empty() {
            "gpt-4.1-mini"
        } else {
            request.model.trim()
        };
        let mut content = vec![json!({ "type": "input_text", "text": request.message })];
        if let (Some(data), Some(mime)) = (
            request.image_base64.as_deref(),
            request.image_mime_type.as_deref(),
        ) {
            content.push(json!({ "type": "input_image", "image_url": format!("data:{mime};base64,{}", strip_data_prefix(data)) }));
        }
        let response = send_json(
            self.client.post(&self.endpoint).bearer_auth(key).json(
                &json!({ "model": model, "input": [{ "role": "user", "content": content }] }),
            ),
            cancellation,
        )
        .await?;
        let text = response
            .get("output_text")
            .and_then(Value::as_str)
            .or_else(|| {
                response
                    .get("output")
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        items.iter().find_map(|item| {
                            item.get("content")
                                .and_then(Value::as_array)
                                .and_then(|content| {
                                    content
                                        .iter()
                                        .find_map(|part| part.get("text").and_then(Value::as_str))
                                })
                        })
                    })
            })
            .ok_or(ProviderError::InvalidResponse)?;
        parse_assistant_response(text)
    }
}

#[derive(Clone)]
pub struct OpenRouterAdapter {
    client: Client,
    endpoint: String,
}

impl OpenRouterAdapter {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            endpoint: "https://openrouter.ai/api/v1/chat/completions".to_owned(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(client: Client, endpoint: &str) -> Self {
        Self {
            client,
            endpoint: endpoint.to_owned(),
        }
    }
}

#[async_trait]
impl ProviderAdapter for OpenRouterAdapter {
    fn name(&self) -> &'static str {
        "openrouter"
    }

    async fn complete(
        &self,
        request: ProviderRequest,
        credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        let key = credential.as_str().trim();
        if key.is_empty() {
            return Err(ProviderError::MissingCredential);
        }
        let model = if request.model.trim().is_empty() {
            "openai/gpt-4.1-mini"
        } else {
            request.model.trim()
        };
        let user_content = if let (Some(data), Some(mime)) = (
            request.image_base64.as_deref(),
            request.image_mime_type.as_deref(),
        ) {
            json!([{ "type": "text", "text": request.message }, { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{}", strip_data_prefix(data)) } }])
        } else {
            json!(request.message)
        };
        let response = send_json(
            self.client.post(&self.endpoint)
                .bearer_auth(key)
                .json(&json!({ "model": model, "messages": [{ "role": "user", "content": user_content }] })),
            cancellation,
        ).await?;
        let text = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| {
                content.as_str().or_else(|| {
                    content.as_array().and_then(|items| {
                        items
                            .iter()
                            .find_map(|part| part.get("text").and_then(Value::as_str))
                    })
                })
            })
            .ok_or(ProviderError::InvalidResponse)?;
        parse_assistant_response(text)
    }
}

async fn send_json(
    builder: reqwest::RequestBuilder,
    cancellation: CancellationToken,
) -> Result<Value, ProviderError> {
    let request = async {
        let response = timeout(DEFAULT_TIMEOUT, builder.send())
            .await
            .map_err(|_| ProviderError::Timeout)?
            .map_err(|_| ProviderError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::HttpStatus(status.as_u16()));
        }
        response
            .json::<Value>()
            .await
            .map_err(|_| ProviderError::InvalidResponse)
    };
    tokio::select! {
        _ = cancellation.cancelled() => Err(ProviderError::Cancelled),
        result = request => result,
    }
}

fn strip_data_prefix(value: &str) -> &str {
    value.split_once(',').map_or(value, |(_, data)| data)
}

pub fn parse_assistant_response(text: &str) -> Result<ProviderResponse, ProviderError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::InvalidResponse);
    }
    let candidate = trimmed
        .strip_prefix("```")
        .and_then(|value| value.strip_suffix("```"))
        .map_or(trimmed, |value| value.trim_start_matches("json").trim());
    if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(candidate) {
        let reply = object
            .get("reply")
            .and_then(Value::as_str)
            .unwrap_or(candidate)
            .trim();
        if reply.is_empty() {
            return Err(ProviderError::InvalidResponse);
        }
        return Ok(ProviderResponse {
            reply: reply.to_owned(),
            emotion: object
                .get("emotion")
                .and_then(Value::as_str)
                .unwrap_or("neutral")
                .to_owned(),
            created_emotion: object
                .get("created_emotion")
                .and_then(Value::as_str)
                .map(str::to_owned),
            actions: object
                .get("actions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        });
    }
    Ok(ProviderResponse {
        reply: trimmed.to_owned(),
        emotion: "neutral".to_owned(),
        created_emotion: None,
        actions: Vec::new(),
    })
}

#[derive(Clone)]
pub struct FakeProvider {
    name: &'static str,
    responses: Arc<Mutex<VecDeque<Result<ProviderResponse, ProviderError>>>>,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    delay: Option<Duration>,
}

impl FakeProvider {
    pub fn new(
        name: &'static str,
        responses: Vec<Result<ProviderResponse, ProviderError>>,
    ) -> Self {
        Self {
            name,
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: None,
        }
    }

    pub fn delayed(name: &'static str, delay: Duration) -> Self {
        Self {
            name,
            responses: Arc::new(Mutex::new(VecDeque::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Some(delay),
        }
    }

    pub async fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().await.clone()
    }
}

#[async_trait]
impl ProviderAdapter for FakeProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn complete(
        &self,
        request: ProviderRequest,
        _credential: &SecretString,
        cancellation: CancellationToken,
    ) -> Result<ProviderResponse, ProviderError> {
        self.requests.lock().await.push(request);
        if let Some(delay) = self.delay {
            tokio::select! {
                _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
                _ = tokio::time::sleep(delay) => {}
            }
        }
        self.responses.lock().await.pop_front().unwrap_or_else(|| {
            Ok(ProviderResponse {
                reply: "fake reply".to_owned(),
                emotion: "neutral".to_owned(),
                created_emotion: None,
                actions: Vec::new(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::ProviderCredentialVault;

    #[test]
    fn parses_aura_json_and_plain_text_without_logging_secrets() {
        let response = parse_assistant_response(
            r#"{"reply":"Done","emotion":"focused","actions":[{"type":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(response.reply, "Done");
        assert_eq!(response.emotion, "focused");
        assert_eq!(response.actions.len(), 1);
        assert_eq!(
            parse_assistant_response("plain reply").unwrap().reply,
            "plain reply"
        );
    }

    #[test]
    fn http_adapters_allow_local_test_endpoints_without_live_provider_calls() {
        let client = Client::new();
        let gemini = GeminiAdapter::with_endpoint(client.clone(), "http://127.0.0.1:1/gemini");
        let openai = OpenAiAdapter::with_endpoint(client.clone(), "http://127.0.0.1:1/openai");
        let openrouter = OpenRouterAdapter::with_endpoint(client, "http://127.0.0.1:1/openrouter");
        assert_eq!(gemini.name(), "gemini");
        assert_eq!(openai.name(), "openai");
        assert_eq!(openrouter.name(), "openrouter");
    }

    #[tokio::test]
    async fn fake_provider_honors_cancellation() {
        let fake = FakeProvider::delayed("gemini", Duration::from_secs(5));
        let token = CancellationToken::new();
        let child = token.clone();
        let vault = ProviderCredentialVault::from_hex_key(&"44".repeat(32)).unwrap();
        let secret = vault.open(&vault.seal("secret").unwrap()).unwrap();
        let task = tokio::spawn(async move {
            fake.complete(
                ProviderRequest {
                    model: String::new(),
                    message: "x".to_owned(),
                    image_base64: None,
                    image_mime_type: None,
                },
                &secret,
                child,
            )
            .await
        });
        token.cancel();
        assert!(matches!(task.await.unwrap(), Err(ProviderError::Cancelled)));
    }
}
