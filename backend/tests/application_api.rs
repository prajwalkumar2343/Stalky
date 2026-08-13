use std::sync::Arc;

use axum::{Extension, Router, body::Body};
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stalky_backend::{
    auth::Principal,
    credentials::ProviderCredentialVault,
    providers::{FakeProvider, ProviderAdapter, ProviderRegistry, ProviderResponse},
    routes::{canonical_routes, canonical_routes_with_runtime},
    store::{InMemoryStore, StoreHandle},
};
use tower::ServiceExt;

const USER_A: &str = "11111111-1111-4111-8111-111111111111";
const USER_B: &str = "22222222-2222-4222-8222-222222222222";

fn app(store: &StoreHandle, user_id: &str) -> Router {
    canonical_routes(store.clone()).layer(Extension(Principal {
        user_id: user_id.to_owned(),
        role: "authenticated".to_owned(),
        aal: None,
        session_id: None,
    }))
}

async fn json_response(response: axum::response::Response) -> Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn durable_resources_are_tenant_scoped_across_compatibility_surfaces() {
    let store: StoreHandle = Arc::new(InMemoryStore::new());

    let created = app(&store, USER_A)
        .oneshot(json_request(
            Method::POST,
            "/memories",
            json!({"title": "Home", "content": "Blue door"}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let memory = json_response(created).await;
    let memory_id = memory["id"].as_str().unwrap();

    let other_user_list = app(&store, USER_B)
        .oneshot(Request::get("/memories").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(other_user_list.status(), StatusCode::OK);
    assert_eq!(json_response(other_user_list).await, json!([]));

    let cross_tenant_delete = app(&store, USER_B)
        .oneshot(
            Request::delete(format!("/memories/{memory_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant_delete.status(), StatusCode::NOT_FOUND);

    let legacy_todo = app(&store, USER_A)
        .oneshot(json_request(
            Method::POST,
            "/todos",
            json!({"title": "Ship Rust backend"}),
        ))
        .await
        .unwrap();
    assert_eq!(legacy_todo.status(), StatusCode::OK);
    let todo = json_response(legacy_todo).await;
    let invalid_update = app(&store, USER_A)
        .oneshot(json_request(
            Method::PATCH,
            &format!("/todos/{}", todo["id"].as_str().unwrap()),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_update.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_runs_are_idempotent_cancellable_and_evented() {
    let store: StoreHandle = Arc::new(InMemoryStore::new());
    let request_body = json!({
        "message": "Plan this",
        "session_id": "33333333-3333-4333-8333-333333333333",
        "provider": "gemini",
        "model": "gemini-test",
        "api_key": "must-not-be-persisted"
    });

    let first = app(&store, USER_A)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/assistant/runs")
                .header("Idempotency-Key", "mobile-request-1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let accepted = json_response(first).await;
    assert_eq!(accepted["state"], "queued");

    let second = app(&store, USER_A)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/assistant/runs")
                .header("Idempotency-Key", "mobile-request-1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::ACCEPTED);
    assert_eq!(json_response(second).await["run_id"], accepted["run_id"]);

    let conflict = app(&store, USER_A)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/assistant/runs")
                .header("Idempotency-Key", "mobile-request-1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"message": "different"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let run_id = accepted["run_id"].as_str().unwrap();
    let foreign_read = app(&store, USER_B)
        .oneshot(
            Request::get(format!("/assistant/runs/{run_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_read.status(), StatusCode::NOT_FOUND);

    let cancelled = app(&store, USER_A)
        .oneshot(
            Request::post(format!("/assistant/runs/{run_id}/cancel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(json_response(cancelled).await["state"], "cancelled");

    let events = app(&store, USER_A)
        .oneshot(
            Request::get(format!("/assistant/runs/{run_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let events = json_response(events).await;
    assert_eq!(events.as_array().unwrap().len(), 2);
    assert_eq!(events[0]["event_type"], "run.queued");
    assert_eq!(events[1]["event_type"], "run.cancelled");
}

#[tokio::test]
async fn assistant_chat_uses_the_rust_fake_provider_without_a_live_call() {
    let store: StoreHandle = Arc::new(InMemoryStore::new());
    let provider = FakeProvider::new(
        "gemini",
        vec![Ok(ProviderResponse {
            reply: "Hello from Rust".to_owned(),
            emotion: "encouraging".to_owned(),
            created_emotion: None,
            actions: Vec::new(),
        })],
    );
    let registry =
        ProviderRegistry::from_adapters([Arc::new(provider) as Arc<dyn ProviderAdapter>]);
    let vault = Arc::new(ProviderCredentialVault::from_hex_key(&"77".repeat(32)).unwrap());
    let app =
        canonical_routes_with_runtime(store, registry, Some(vault)).layer(Extension(Principal {
            user_id: USER_A.to_owned(),
            role: "authenticated".to_owned(),
            aal: None,
            session_id: None,
        }));
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/assistant/chat",
            json!({"message": "Say hello", "provider": "gemini", "api_key": "redact-me"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["reply"], "Hello from Rust");
    assert_eq!(body["emotion"], "encouraging");
    assert!(!body.to_string().contains("redact-me"));
}

#[tokio::test]
async fn mini_app_records_accept_scalars_and_reject_nested_payloads() {
    let store: StoreHandle = Arc::new(InMemoryStore::new());
    let created = app(&store, USER_A)
        .oneshot(json_request(
            Method::POST,
            "/mini-apps/habit-tracker/records",
            json!({"recordType": "habit", "values": {"name": "Run", "count": 1, "done": true}}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let nested = app(&store, USER_A)
        .oneshot(json_request(
            Method::POST,
            "/mini-apps/habit-tracker/records",
            json!({"values": {"nested": {"unsafe": true}}}),
        ))
        .await
        .unwrap();
    assert_eq!(nested.status(), StatusCode::BAD_REQUEST);
}
