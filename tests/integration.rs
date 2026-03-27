use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use bytus::routes;
use bytus::state::AppState;

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_state(pool: PgPool) -> AppState {
    AppState {
        pool,
        jwt_secret: "test_jwt_secret_must_be_32_chars!!".to_string(),
        fee_bps: 50,
    }
}

/// Fire a single request against the app. Consumes the (cloned) router.
async fn send(
    app: axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);

    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {t}"));
    }

    let req_body = match body {
        Some(b) => {
            builder = builder.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&b).unwrap())
        }
        None => Body::from(""),
    };

    let resp = app.oneshot(builder.body(req_body).unwrap()).await.unwrap();

    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Signup and return (token, user_id).
async fn signup_and_login(app: &axum::Router, email: &str) -> (String, Uuid) {
    let (_, body) = send(
        app.clone(),
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": email, "password": "password123" })),
        None,
    )
    .await;
    let token = body["token"].as_str().unwrap().to_string();
    let user_id = Uuid::parse_str(body["user_id"].as_str().unwrap()).unwrap();
    (token, user_id)
}

// ── health ────────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn test_health(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, body) = send(app, Method::GET, "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

// ── auth ──────────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn test_signup_returns_user_id_and_token(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, body) = send(
        app,
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": "new@test.com", "password": "password123" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["user_id"].is_string());
    assert!(body["token"].is_string());
}

#[sqlx::test(migrations = "./migrations")]
async fn test_signup_duplicate_email_returns_conflict(pool: PgPool) {
    let app = routes::app(make_state(pool));
    send(
        app.clone(),
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": "dup@test.com", "password": "pass" })),
        None,
    )
    .await;
    let (status, _) = send(
        app,
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": "dup@test.com", "password": "pass" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_login_returns_token(pool: PgPool) {
    let app = routes::app(make_state(pool));
    send(
        app.clone(),
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": "login@test.com", "password": "password123" })),
        None,
    )
    .await;
    let (status, body) = send(
        app,
        Method::POST,
        "/api/auth/login",
        Some(json!({ "email": "login@test.com", "password": "password123" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["token"].is_string());
}

#[sqlx::test(migrations = "./migrations")]
async fn test_login_wrong_password_returns_unauthorized(pool: PgPool) {
    let app = routes::app(make_state(pool));
    send(
        app.clone(),
        Method::POST,
        "/api/auth/signup",
        Some(json!({ "email": "wp@test.com", "password": "correct" })),
        None,
    )
    .await;
    let (status, _) = send(
        app,
        Method::POST,
        "/api/auth/login",
        Some(json!({ "email": "wp@test.com", "password": "wrong" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_me_without_token_returns_unauthorized(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, _) = send(app, Method::GET, "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_me_with_token_returns_user(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (token, _) = signup_and_login(&app, "me@test.com").await;
    let (status, body) = send(app, Method::GET, "/api/me", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "me@test.com");
    assert_eq!(body["kyc_status"], "none");
}

// ── settlements ───────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_requires_auth(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, _) = send(
        app,
        Method::POST,
        "/api/settlements",
        Some(json!({ "gross_amount": 10000, "idempotency_key": "k1" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Happy path — verifies all 5 transaction steps via DB queries.
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_happy_path(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "happy@test.com").await;

    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({
            "gross_amount": 10000,
            "currency": "USD",
            "idempotency_key": "happy-001"
        })),
        Some(&token),
    )
    .await;

    // Response
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["gross_amount"], 10000);
    assert_eq!(body["fee_amount"], 50); // 10000 * 50 / 10000
    assert_eq!(body["net_amount"], 9950);
    assert_eq!(body["status"], "completed");
    assert_eq!(body["currency"], "USD");

    let settlement_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    // Step 2+3 — two ledger entries.
    let ledger_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM ledger_entries WHERE settlement_id = $1 AND user_id = $2",
        settlement_id,
        user_id,
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(ledger_count, 2);

    // Confirm credit and fee entries individually.
    let credit_amt = sqlx::query_scalar!(
        "SELECT amount FROM ledger_entries WHERE settlement_id = $1 AND entry_type = 'credit'",
        settlement_id,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(credit_amt, 10000);

    let fee_amt = sqlx::query_scalar!(
        "SELECT amount FROM ledger_entries WHERE settlement_id = $1 AND entry_type = 'fee'",
        settlement_id,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(fee_amt, -50);

    // Step 4 — apply_marker exists.
    let marker_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM apply_markers WHERE settlement_id = $1",
        settlement_id,
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(marker_count, 1);

    // Step 5 — balance updated.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM balances WHERE user_id = $1 AND currency = 'USD'",
        user_id,
    )
    .fetch_optional(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(balance, 9950);

    Ok(())
}

/// Idempotency — same key twice returns identical response, no duplicate writes.
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_idempotency(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "idem@test.com").await;

    let payload = json!({
        "gross_amount": 5000,
        "currency": "USD",
        "idempotency_key": "idem-001"
    });

    let (s1, b1) = send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(payload.clone()),
        Some(&token),
    )
    .await;
    let (s2, b2) = send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(payload),
        Some(&token),
    )
    .await;

    // Both succeed with the same settlement ID.
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b1["id"], b2["id"]);

    // Exactly one settlement in DB.
    let settlement_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM settlements WHERE user_id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(settlement_count, 1);

    // Exactly two ledger entries (credit + fee), not four.
    let ledger_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM ledger_entries WHERE user_id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(ledger_count, 2);

    // Balance reflects exactly one net amount.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM balances WHERE user_id = $1 AND currency = 'USD'",
        user_id,
    )
    .fetch_optional(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(balance, 5000 - 5000 * 50 / 10_000);

    Ok(())
}

/// Concurrent idempotency — two concurrent requests with the same key produce one settlement.
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_concurrent_idempotency(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "concurrent@test.com").await;

    let payload = json!({
        "gross_amount": 8000,
        "currency": "USD",
        "idempotency_key": "concurrent-001"
    });

    let (r1, r2) = tokio::join!(
        send(
            app.clone(),
            Method::POST,
            "/api/settlements",
            Some(payload.clone()),
            Some(&token)
        ),
        send(
            app.clone(),
            Method::POST,
            "/api/settlements",
            Some(payload),
            Some(&token)
        ),
    );

    assert_eq!(r1.0, StatusCode::OK);
    assert_eq!(r2.0, StatusCode::OK);
    // Both return the same settlement ID.
    assert_eq!(r1.1["id"], r2.1["id"]);

    // Exactly one settlement and one apply_marker in DB.
    let settlement_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM settlements WHERE user_id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(settlement_count, 1);

    let marker_count = sqlx::query_scalar!("SELECT COUNT(*) FROM apply_markers",)
        .fetch_one(&pool)
        .await?
        .unwrap_or(0);
    assert_eq!(marker_count, 1);

    Ok(())
}

/// Balance accuracy — 10 settlements, balance must equal sum of all net amounts.
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_balance_accuracy(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "balance@test.com").await;

    let mut expected_balance: i64 = 0;

    for i in 0..10_i64 {
        let gross = 10_000 + i * 1_000; // 10000, 11000, ... 19000
        let fee = gross * 50 / 10_000;
        let net = gross - fee;
        expected_balance += net;

        let (status, _) = send(
            app.clone(),
            Method::POST,
            "/api/settlements",
            Some(json!({
                "gross_amount": gross,
                "currency": "USD",
                "idempotency_key": format!("bal-{i}")
            })),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "settlement {i} failed");
    }

    let actual_balance = sqlx::query_scalar!(
        "SELECT amount FROM balances WHERE user_id = $1 AND currency = 'USD'",
        user_id,
    )
    .fetch_optional(&pool)
    .await?
    .unwrap_or(0);

    assert_eq!(actual_balance, expected_balance);

    // GET /api/settlements returns all 10.
    let (status, list) = send(app, Method::GET, "/api/settlements", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 10);

    Ok(())
}
