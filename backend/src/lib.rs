pub mod auth;
pub mod config;
pub mod error;

use std::time::Duration;

use axum::{Extension, Json, Router, middleware, routing::get};
use http::{HeaderName, header};
use serde::Serialize;
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::{
    auth::{AuthState, Principal},
    config::Config,
    error::REQUEST_ID_HEADER,
};

#[derive(Serialize)]
struct Health {
    service: &'static str,
    version: &'static str,
    status: &'static str,
}

pub fn app(config: Config) -> Router {
    let auth_state = AuthState::new(&config.supabase_url)
        .expect("validated Supabase configuration must create an auth client");
    app_with_auth(auth_state)
}

fn app_with_auth(auth_state: AuthState) -> Router {
    let request_id = HeaderName::from_static(REQUEST_ID_HEADER);
    let middleware = ServiceBuilder::new()
        .layer(SetSensitiveRequestHeadersLayer::new(std::iter::once(
            header::AUTHORIZATION,
        )))
        .layer(SetRequestIdLayer::new(request_id.clone(), MakeRequestUuid))
        .layer(PropagateRequestIdLayer::new(request_id))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            http::StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(30),
        ));

    let protected =
        Router::new()
            .route("/me", get(me))
            .route_layer(middleware::from_fn_with_state(
                auth_state,
                auth::require_auth,
            ));

    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .nest("/v1", protected)
        .fallback(not_found)
        .layer(middleware::from_fn(error::attach_request_id))
        .layer(middleware)
        .layer(middleware::from_fn(error::normalize_problem_response))
}

async fn me(Extension(principal): Extension<Principal>) -> Json<Principal> {
    Json(principal)
}

async fn not_found() -> error::AppError {
    error::AppError::not_found()
}

async fn live() -> Json<Health> {
    Json(health())
}

async fn ready() -> Json<Health> {
    Json(health())
}

fn health() -> Health {
    Health {
        service: "stalky-backend",
        version: env!("CARGO_PKG_VERSION"),
        status: "ok",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use tower::ServiceExt;
    use url::Url;

    fn test_app() -> Router {
        app(Config {
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            supabase_url: Url::parse("https://project.supabase.co").unwrap(),
        })
    }

    #[tokio::test]
    async fn health_is_public_and_protected_routes_require_bearer_auth() {
        let health = test_app()
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let unauthorized = test_app()
            .oneshot(Request::get("/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .unwrap(),
            "Bearer"
        );
        assert!(unauthorized.headers().contains_key(REQUEST_ID_HEADER));
        let body = unauthorized.into_body().collect().await.unwrap().to_bytes();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "auth.unauthorized");
        assert!(problem["requestId"].is_string());
    }
}
