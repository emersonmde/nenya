//! HTTP API integration tests
//!
//! Tests the HTTP server by spawning in-process server instances and making HTTP requests.

#[cfg(feature = "server")]
mod integration_tests {
    use nenya::api::{self, AppState, RateLimitManager};
    use reqwest::{Client, StatusCode};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::RwLock;
    use tokio::task::JoinHandle;
    use tokio::time::sleep;

    static NEXT_PORT: AtomicU16 = AtomicU16::new(9000);

    fn allocate_test_port() -> u16 {
        NEXT_PORT.fetch_add(1, Ordering::SeqCst)
    }

    struct TestServer {
        port: u16,
        _handle: JoinHandle<()>,
    }

    impl TestServer {
        async fn start(port: u16, target_rate: f64) -> Self {
            // Create manager with config
            let manager = RateLimitManager::new(target_rate, 0.8, 0.05, 0.04);

            // Create app state
            let state = Arc::new(AppState {
                manager: Arc::new(RwLock::new(manager)),
                metrics: api::MetricsCollector::new(),
            });

            // Create router
            let router = api::create_router(state);

            // Bind to port
            let addr = format!("127.0.0.1:{}", port);
            let listener = TcpListener::bind(&addr)
                .await
                .expect("Failed to bind to port");

            // Spawn server in background
            let handle = tokio::spawn(async move {
                axum::serve(listener, router).await.expect("Server failed");
            });

            // Wait for server to be ready
            sleep(Duration::from_millis(100)).await;

            TestServer {
                port,
                _handle: handle,
            }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{}", self.port, path)
        }
    }

    #[tokio::test]
    async fn test_server_startup_and_health() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();
        let response = client
            .get(server.url("/health"))
            .send()
            .await
            .expect("Failed to get health");

        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response.json().await.expect("Failed to parse JSON");
        assert_eq!(body["healthy"], true);
        assert_eq!(body["scopes"], 0);
        assert_eq!(body["peers"], 0);
    }

    #[tokio::test]
    async fn test_should_throttle_endpoint() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        // First request should not throttle
        let response = client
            .post(server.url("/should_throttle"))
            .json(&json!({
                "scope": "test-scope"
            }))
            .send()
            .await
            .expect("Failed to post");

        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = response.json().await.expect("Failed to parse JSON");
        assert_eq!(body["should_throttle"], false);
        assert!(body["local_accepted_rate"].is_number());
        assert!(body["refill_rate"].is_number());
        assert_eq!(body["num_peers"], 0);
    }

    #[tokio::test]
    async fn test_scope_auto_creation() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        // Create multiple scopes
        for scope in &["scope-1", "scope-2", "scope-3"] {
            client
                .post(server.url("/should_throttle"))
                .json(&json!({
                    "scope": scope
                }))
                .send()
                .await
                .expect("Failed to post");
        }

        // Check health endpoint reports correct count
        let response = client
            .get(server.url("/health"))
            .send()
            .await
            .expect("Failed to get health");

        let body: Value = response.json().await.expect("Failed to parse JSON");
        assert_eq!(body["scopes"], 3);
    }

    #[tokio::test]
    async fn test_empty_scope_rejected() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        let response = client
            .post(server.url("/should_throttle"))
            .json(&json!({
                "scope": ""
            }))
            .send()
            .await
            .expect("Failed to post");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body: Value = response.json().await.expect("Failed to parse JSON");
        assert!(body["error"].is_string());
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("scope cannot be empty"));
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        // Generate some traffic
        for _ in 0..5 {
            client
                .post(server.url("/should_throttle"))
                .json(&json!({
                    "scope": "test"
                }))
                .send()
                .await
                .expect("Failed to post");
        }

        // Check metrics
        let response = client
            .get(server.url("/metrics"))
            .send()
            .await
            .expect("Failed to get metrics");

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.text().await.expect("Failed to get text");
        assert!(body.contains("nenya_requests_total"));
        assert!(body.contains("nenya_requests_accepted"));
        assert!(body.contains("nenya_requests_throttled"));
        assert!(body.contains("scope=\"test\""));
    }

    #[tokio::test]
    async fn test_rate_limiting_behavior() {
        let port = allocate_test_port();

        // Set very low rate for testing
        let server = TestServer::start(port, 5.0).await;

        let client = Client::new();

        let mut accepted = 0;
        let mut throttled = 0;

        // Make many requests quickly
        for _ in 0..20 {
            let response = client
                .post(server.url("/should_throttle"))
                .json(&json!({
                    "scope": "test"
                }))
                .send()
                .await
                .expect("Failed to post");

            let body: Value = response.json().await.expect("Failed to parse JSON");

            if body["should_throttle"] == false {
                accepted += 1;
            } else {
                throttled += 1;
            }
        }

        // With rate of 5/sec, we should see some throttling
        assert!(throttled > 0, "Expected some requests to be throttled");
        assert!(accepted > 0, "Expected some requests to be accepted");
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        // Make concurrent requests
        let mut handles = vec![];
        for i in 0..10 {
            let client = client.clone();
            let url = server.url("/should_throttle");
            let scope = format!("scope-{}", i);

            let handle = tokio::spawn(async move {
                client
                    .post(&url)
                    .json(&json!({
                        "scope": scope
                    }))
                    .send()
                    .await
                    .expect("Failed to post")
            });

            handles.push(handle);
        }

        // Wait for all requests to complete
        for handle in handles {
            let response = handle.await.expect("Task panicked");
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Verify all scopes were created
        let response = client
            .get(server.url("/health"))
            .send()
            .await
            .expect("Failed to get health");

        let body: Value = response.json().await.expect("Failed to parse JSON");
        assert_eq!(body["scopes"], 10);
    }

    #[tokio::test]
    async fn test_invalid_json_rejected() {
        let port = allocate_test_port();
        let server = TestServer::start(port, 100.0).await;

        let client = Client::new();

        let response = client
            .post(server.url("/should_throttle"))
            .body("invalid json")
            .send()
            .await
            .expect("Failed to post");

        // Axum returns 415 Unsupported Media Type for invalid JSON
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
}
