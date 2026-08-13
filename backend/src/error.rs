use axum::{
    Json,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    code: &'static str,
    title: &'static str,
    detail: String,
    retryable: bool,
    request_id: Option<String>,
    www_authenticate: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Problem {
    #[serde(rename = "type")]
    problem_type: String,
    status: u16,
    code: &'static str,
    title: &'static str,
    detail: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

impl AppError {
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "route.not_found",
            title: "Route not found",
            detail: "The requested API route does not exist.".to_owned(),
            retryable: false,
            request_id: None,
            www_authenticate: None,
        }
    }

    /// Generic 401 used for every authentication failure. The body never
    /// discloses why verification failed.
    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "auth.unauthorized",
            title: "Unauthorized",
            detail: "Valid credentials are required to access this resource.".to_owned(),
            retryable: false,
            request_id: None,
            www_authenticate: Some("Bearer"),
        }
    }

    fn for_status(status: StatusCode) -> Option<Self> {
        let (code, title, detail, retryable) = match status {
            StatusCode::PAYLOAD_TOO_LARGE => (
                "request.too_large",
                "Payload too large",
                "The request body exceeds the 1 MiB limit.".to_owned(),
                false,
            ),
            StatusCode::TOO_MANY_REQUESTS => (
                "rate.limited",
                "Rate limited",
                "Too many requests. Retry after the suggested delay.".to_owned(),
                true,
            ),
            StatusCode::INTERNAL_SERVER_ERROR => (
                "server.internal",
                "Internal server error",
                "An unexpected error occurred on the server.".to_owned(),
                true,
            ),
            StatusCode::GATEWAY_TIMEOUT => (
                "server.timeout",
                "Gateway timeout",
                "The request did not complete before the server timeout.".to_owned(),
                true,
            ),
            _ => return None,
        };
        Some(Self {
            status,
            code,
            title,
            detail,
            retryable,
            request_id: None,
            www_authenticate: None,
        })
    }

    pub(crate) fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    pub fn bad_request(code: &'static str, detail: impl Into<String>) -> Self {
        Self::custom(
            StatusCode::BAD_REQUEST,
            code,
            "Invalid request",
            detail,
            false,
        )
    }

    pub fn conflict(code: &'static str, detail: impl Into<String>) -> Self {
        Self::custom(StatusCode::CONFLICT, code, "Conflict", detail, false)
    }

    pub fn not_implemented(code: &'static str, detail: impl Into<String>) -> Self {
        Self::custom(
            StatusCode::NOT_IMPLEMENTED,
            code,
            "Not implemented",
            detail,
            false,
        )
    }

    pub fn resource_not_found(resource: &'static str) -> Self {
        Self::custom(
            StatusCode::NOT_FOUND,
            "resource.not_found",
            "Resource not found",
            format!("{resource} was not found."),
            false,
        )
    }

    pub fn storage_unavailable() -> Self {
        Self::custom(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage.unavailable",
            "Storage unavailable",
            "The persistence service is temporarily unavailable.",
            true,
        )
    }

    pub fn service_unavailable(code: &'static str, detail: impl Into<String>) -> Self {
        Self::custom(
            StatusCode::SERVICE_UNAVAILABLE,
            code,
            "Service unavailable",
            detail,
            true,
        )
    }

    pub fn internal() -> Self {
        Self::custom(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server.internal",
            "Internal server error",
            "An unexpected error occurred on the server.",
            true,
        )
    }

    fn custom(
        status: StatusCode,
        code: &'static str,
        title: &'static str,
        detail: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            code,
            title,
            detail: detail.into(),
            retryable,
            request_id: None,
            www_authenticate: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let problem = Problem {
            problem_type: format!("https://api.stalky.app/problems/{}", self.code),
            status: self.status.as_u16(),
            code: self.code,
            title: self.title,
            detail: self.detail,
            retryable: self.retryable,
            request_id: self.request_id,
        };
        let request_id = problem.request_id.clone();
        let mut response = (self.status, Json(problem)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(scheme) = self.www_authenticate {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(scheme));
        }
        if let Some(request_id) = request_id
            && let Ok(value) = HeaderValue::from_str(&request_id)
        {
            response.headers_mut().insert(REQUEST_ID_HEADER, value);
        }
        response
    }
}

pub async fn normalize_problem_response(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let is_problem = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value == "application/problem+json");
    if is_problem {
        return response;
    }
    let Some(error) = AppError::for_status(response.status()) else {
        return response;
    };
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    error.with_request_id(request_id).into_response()
}

pub async fn attach_request_id(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let response = next.run(request).await;
    if let Some(request_id) = request_id
        && response.status() == StatusCode::NOT_FOUND
    {
        return AppError::not_found()
            .with_request_id(Some(request_id))
            .into_response();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::AppError;
    use axum::response::IntoResponse;
    use http::{StatusCode, header};
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn problem_response_has_stable_shape_and_content_type() {
        let response = AppError::not_found()
            .with_request_id(Some("req-test".to_owned()))
            .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "route.not_found");
        assert_eq!(problem["requestId"], "req-test");
        assert_eq!(problem["retryable"], false);
    }
}
