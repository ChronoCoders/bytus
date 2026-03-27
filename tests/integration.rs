use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use bytus::chain;
use bytus::routes;
use bytus::state::AppState;

/// Seed BYTS balance for a user directly via DB. Used in BYTS tests.
async fn seed_byts(pool: &PgPool, user_id: Uuid, amount: i64) {
    sqlx::query!(
        r#"INSERT INTO byts_balances (user_id, amount) VALUES ($1, $2)
           ON CONFLICT (user_id) DO UPDATE SET amount = $2"#,
        user_id,
        amount,
    )
    .execute(pool)
    .await
    .unwrap();
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_state(pool: PgPool) -> AppState {
    AppState {
        pool,
        jwt_secret: "test_jwt_secret_must_be_32_chars!!".to_string(),
        fee_bps: 50,
        // 2 BYTS per fiat fee unit (in micro-units). Chosen to differ from
        // fiat_fee so accidental equality cannot mask a computation bug.
        byts_rate: 2_000_000,
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

// ── settlement chain events ───────────────────────────────────────────────────

// ── BYTS fee ──────────────────────────────────────────────────────────────────

/// Settlement response includes byts_fee matching the formula:
/// byts_fee = fiat_fee * byts_rate / 1_000_000
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_byts_fee(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "bytsfee@test.com").await;

    // fee_bps = 50, byts_rate = 2_000_000 (from make_state)
    // gross = 10_000 → fiat_fee = 10_000 * 50 / 10_000 = 50
    // byts_fee = 50 * 2_000_000 / 1_000_000 = 100
    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({
            "gross_amount": 10000,
            "currency": "USD",
            "idempotency_key": "byts-fee-001"
        })),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fee_amount"], 50);
    assert_eq!(body["net_amount"], 9950);
    assert_eq!(
        body["byts_fee"], 100,
        "byts_fee must equal fiat_fee * byts_rate / 1_000_000"
    );

    // Verify byts_fee is persisted in DB.
    let settlement_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    let db_byts_fee = sqlx::query_scalar!(
        "SELECT byts_fee FROM settlements WHERE id = $1 AND user_id = $2",
        settlement_id,
        user_id,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(db_byts_fee, 100);

    // Idempotent replay returns the same byts_fee.
    let (_, body2) = send(
        app,
        Method::POST,
        "/api/settlements",
        Some(json!({
            "gross_amount": 10000,
            "currency": "USD",
            "idempotency_key": "byts-fee-001"
        })),
        Some(&token),
    )
    .await;
    assert_eq!(
        body2["byts_fee"], 100,
        "idempotent replay must return same byts_fee"
    );

    Ok(())
}

// ── Phase 2 auth guards ───────────────────────────────────────────────────────

/// GET /api/byts/balance requires authentication.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_balance_requires_auth(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, _) = send(app, Method::GET, "/api/byts/balance", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// GET /api/byts/lock requires authentication.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_lock_list_requires_auth(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, _) = send(app, Method::GET, "/api/byts/lock", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// GET /api/chain/blocks requires authentication.
#[sqlx::test(migrations = "./migrations")]
async fn test_chain_blocks_requires_auth(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (status, _) = send(app, Method::GET, "/api/chain/blocks", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── Phase 2 baseline ──────────────────────────────────────────────────────────

/// GET /api/byts/balance returns { "amount": 0 } when no balance record exists.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_balance_zero_for_new_user(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (token, _) = signup_and_login(&app, "zero@test.com").await;
    let (status, body) = send(app, Method::GET, "/api/byts/balance", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["amount"], 0);
}

// ── deployment-validator smoke test 15 ───────────────────────────────────────

/// verify_block passes for every block after a full settlement + lock + unlock cycle.
///
/// This is the chain integrity check from the deployment-validator:
/// "Run verify_block() on all blocks → All blocks return Ok(())"
#[sqlx::test(migrations = "./migrations")]
async fn test_chain_verify_all_blocks(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "integrity@test.com").await;
    seed_byts(&pool, user_id, 10_000).await;

    // Block 2 — settlement chain event.
    send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({ "gross_amount": 20000, "idempotency_key": "integrity-001" })),
        Some(&token),
    )
    .await;

    // Block 3 — lock chain event.
    let (_, lock_body) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 3000 })),
        Some(&token),
    )
    .await;
    let lock_id = lock_body["lock_id"].as_str().unwrap().to_string();

    // Block 4 — unlock chain event.
    send(
        app,
        Method::POST,
        "/api/byts/unlock",
        Some(json!({ "lock_id": lock_id })),
        Some(&token),
    )
    .await;

    // Genesis + 3 operation blocks = 4 total.
    let block_ids: Vec<i64> = sqlx::query_scalar!("SELECT id FROM blocks ORDER BY id")
        .fetch_all(&pool)
        .await?;
    assert_eq!(
        block_ids.len(),
        4,
        "expected genesis + settlement + lock + unlock blocks"
    );

    // Every block must pass hash verification.
    for block_id in block_ids {
        chain::verify_block(block_id, &pool)
            .await
            .unwrap_or_else(|e| panic!("block {block_id} failed verify_block: {e}"));
    }

    Ok(())
}

// ── chain API ─────────────────────────────────────────────────────────────────

/// GET /api/chain/blocks returns list including genesis and settlement blocks.
#[sqlx::test(migrations = "./migrations")]
async fn test_chain_list_blocks(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, _) = signup_and_login(&app, "chainlist@test.com").await;

    // Create a settlement so a non-genesis block exists.
    send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({ "gross_amount": 5000, "idempotency_key": "cl-001" })),
        Some(&token),
    )
    .await;

    let (status, body) = send(app, Method::GET, "/api/chain/blocks", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);

    let blocks = body.as_array().unwrap();
    assert!(
        blocks.len() >= 2,
        "expected genesis block + at least 1 settlement block"
    );

    // Most recent block first — it's the settlement block.
    let settlement_block = &blocks[0];
    assert!(settlement_block["id"].as_i64().unwrap() > 1);
    assert_eq!(settlement_block["event_count"], 1);
    assert!(settlement_block["hash"].as_str().unwrap().len() == 64); // 32 bytes hex
    assert!(settlement_block["prev_hash"].as_str().unwrap().len() == 64);

    // Genesis block is last in the list (lowest id).
    let genesis = blocks.last().unwrap();
    assert_eq!(genesis["id"], 1);
    assert_eq!(
        genesis["prev_hash"].as_str().unwrap(),
        "0000000000000000000000000000000000000000000000000000000000000000"
    );

    Ok(())
}

/// GET /api/chain/blocks/:id returns block detail with events.
#[sqlx::test(migrations = "./migrations")]
async fn test_chain_get_block(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, _) = signup_and_login(&app, "chainget@test.com").await;

    send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({ "gross_amount": 8000, "idempotency_key": "cg-001" })),
        Some(&token),
    )
    .await;

    // Find the block id for the settlement event from DB.
    let block_id = sqlx::query_scalar!(
        "SELECT block_id FROM chain_events WHERE event_type = 'settlement' ORDER BY id DESC LIMIT 1"
    )
    .fetch_one(&pool)
    .await?;

    let (status, body) = send(
        app,
        Method::GET,
        &format!("/api/chain/blocks/{block_id}"),
        None,
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], block_id);
    assert!(body["hash"].as_str().unwrap().len() == 64);
    assert!(body["prev_hash"].as_str().unwrap().len() == 64);

    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event_type"], "settlement");
    assert!(events[0]["payload"]["settlement_id"].is_string());
    assert_eq!(events[0]["payload"]["gross_amount"], 8000);

    Ok(())
}

/// GET /api/chain/blocks/:id returns 404 for a non-existent block.
#[sqlx::test(migrations = "./migrations")]
async fn test_chain_block_not_found(pool: PgPool) {
    let app = routes::app(make_state(pool));
    let (token, _) = signup_and_login(&app, "chain404@test.com").await;

    let (status, _) = send(
        app,
        Method::GET,
        "/api/chain/blocks/999999",
        None,
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Every new settlement must produce exactly one chain event of type 'settlement'.
#[sqlx::test(migrations = "./migrations")]
async fn test_settlement_produces_chain_event(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, _) = signup_and_login(&app, "chain@test.com").await;

    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/api/settlements",
        Some(json!({
            "gross_amount": 10000,
            "currency": "USD",
            "idempotency_key": "chain-evt-001"
        })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let settlement_id = body["id"].as_str().unwrap().to_string();

    // Exactly one 'settlement' chain event in DB.
    let event_count =
        sqlx::query_scalar!("SELECT COUNT(*) FROM chain_events WHERE event_type = 'settlement'")
            .fetch_one(&pool)
            .await?
            .unwrap_or(0);
    assert_eq!(event_count, 1);

    // The event payload references this settlement.
    let payload = sqlx::query_scalar!(
        "SELECT payload FROM chain_events WHERE event_type = 'settlement' LIMIT 1"
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        payload["settlement_id"].as_str().unwrap(),
        settlement_id,
        "chain event payload must reference the settlement id"
    );

    // A block was written (genesis + 1 settlement block).
    let block_count = sqlx::query_scalar!("SELECT COUNT(*) FROM blocks")
        .fetch_one(&pool)
        .await?
        .unwrap_or(0);
    assert_eq!(block_count, 2);

    // Idempotent replay does NOT produce a second chain event.
    let (status2, body2) = send(
        app,
        Method::POST,
        "/api/settlements",
        Some(json!({
            "gross_amount": 10000,
            "currency": "USD",
            "idempotency_key": "chain-evt-001"
        })),
        Some(&token),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(
        body2["id"], body["id"],
        "idempotent replay must return same settlement id"
    );

    let event_count_after =
        sqlx::query_scalar!("SELECT COUNT(*) FROM chain_events WHERE event_type = 'settlement'")
            .fetch_one(&pool)
            .await?
            .unwrap_or(0);
    assert_eq!(
        event_count_after, 1,
        "idempotent replay must not produce a second chain event"
    );

    Ok(())
}

// ── BYTS lock / unlock ────────────────────────────────────────────────────────

/// Lock happy path: balance deducted, lock active, chain event written.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_lock_happy_path(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "lock@test.com").await;
    seed_byts(&pool, user_id, 5000).await;

    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 1000 })),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["lock_id"].is_string());
    assert_eq!(body["amount"], 1000);
    assert_eq!(body["status"], "active");

    let lock_id = Uuid::parse_str(body["lock_id"].as_str().unwrap()).unwrap();

    // Balance deducted.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(balance, 4000);

    // Lock record is active in DB.
    let lock_status = sqlx::query_scalar!(
        "SELECT status FROM byts_locks WHERE id = $1 AND user_id = $2",
        lock_id,
        user_id,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(lock_status, "active");

    // Chain event was written.
    let event_count =
        sqlx::query_scalar!("SELECT COUNT(*) FROM chain_events WHERE event_type = 'lock'")
            .fetch_one(&pool)
            .await?
            .unwrap_or(0);
    assert_eq!(event_count, 1);

    // GET /api/byts/lock lists the lock.
    let (status, list) = send(
        app.clone(),
        Method::GET,
        "/api/byts/lock",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let locks = list.as_array().unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0]["lock_id"], body["lock_id"]);

    // GET /api/byts/balance reflects the deduction.
    let (status, bal) = send(app, Method::GET, "/api/byts/balance", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bal["amount"], 4000);

    Ok(())
}

/// Unlock: balance restored, lock released, chain event written.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_unlock(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "unlock@test.com").await;
    seed_byts(&pool, user_id, 5000).await;

    // Lock first.
    let (_, lock_body) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 2000 })),
        Some(&token),
    )
    .await;
    let lock_id = lock_body["lock_id"].as_str().unwrap().to_string();

    // Unlock.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/api/byts/unlock",
        Some(json!({ "lock_id": lock_id })),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["lock_id"], lock_id);
    assert_eq!(body["status"], "released");

    // Balance fully restored.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(balance, 5000);

    // Lock is released in DB.
    let lock_status = sqlx::query_scalar!(
        "SELECT status FROM byts_locks WHERE id = $1",
        Uuid::parse_str(&lock_id).unwrap(),
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(lock_status, "released");

    // Unlock chain event was written.
    let event_count =
        sqlx::query_scalar!("SELECT COUNT(*) FROM chain_events WHERE event_type = 'unlock'")
            .fetch_one(&pool)
            .await?
            .unwrap_or(0);
    assert_eq!(event_count, 1);

    // Cannot unlock the same lock again — 422.
    let (status, _) = send(
        app,
        Method::POST,
        "/api/byts/unlock",
        Some(json!({ "lock_id": lock_id })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    Ok(())
}

/// Insufficient balance returns 422 with no partial state.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_lock_insufficient_balance(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "insuf@test.com").await;
    seed_byts(&pool, user_id, 100).await;

    let (status, body) = send(
        app,
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 1000 })),
        Some(&token),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("insufficient"));

    // No lock record created.
    let lock_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM byts_locks WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(lock_count, 0);

    // Balance unchanged.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(balance, 100);

    Ok(())
}

/// Double-lock: two sequential locks succeed while balance allows, third fails.
#[sqlx::test(migrations = "./migrations")]
async fn test_byts_double_lock(pool: PgPool) -> sqlx::Result<()> {
    let app = routes::app(make_state(pool.clone()));
    let (token, user_id) = signup_and_login(&app, "double@test.com").await;
    seed_byts(&pool, user_id, 2000).await;

    // First lock: 1000 — succeeds (balance: 1000).
    let (s1, _) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 1000 })),
        Some(&token),
    )
    .await;
    assert_eq!(s1, StatusCode::OK);

    // Second lock: 1000 — succeeds (balance: 0).
    let (s2, _) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 1000 })),
        Some(&token),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);

    // Third lock: 1 — fails, balance exhausted.
    let (s3, _) = send(
        app.clone(),
        Method::POST,
        "/api/byts/lock",
        Some(json!({ "amount": 1 })),
        Some(&token),
    )
    .await;
    assert_eq!(s3, StatusCode::UNPROCESSABLE_ENTITY);

    // Two active locks in DB.
    let lock_count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM byts_locks WHERE user_id = $1 AND status = 'active'",
        user_id
    )
    .fetch_one(&pool)
    .await?
    .unwrap_or(0);
    assert_eq!(lock_count, 2);

    // Balance is exactly 0.
    let balance = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(balance, 0);

    Ok(())
}
