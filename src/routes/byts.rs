use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::chain;
use crate::error::AppError;
use crate::models::BytsLock;
use crate::state::AppState;

// ── response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct BytsBalanceResponse {
    pub amount: i64,
}

#[derive(Serialize)]
pub struct LockResponse {
    pub lock_id: Uuid,
    pub amount: i64,
    pub status: String,
}

#[derive(Serialize)]
pub struct UnlockResponse {
    pub lock_id: Uuid,
    pub status: String,
}

#[derive(Serialize)]
pub struct LockDetailResponse {
    pub lock_id: Uuid,
    pub amount: i64,
    pub locked_at: DateTime<Utc>,
    pub unlocked_at: Option<DateTime<Utc>>,
    pub status: String,
}

impl From<BytsLock> for LockDetailResponse {
    fn from(l: BytsLock) -> Self {
        Self {
            lock_id: l.id,
            amount: l.amount,
            locked_at: l.locked_at,
            unlocked_at: l.unlocked_at,
            status: l.status,
        }
    }
}

// ── request types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LockRequest {
    pub amount: i64,
}

#[derive(Deserialize)]
pub struct UnlockRequest {
    pub lock_id: Uuid,
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// GET /api/byts/balance
///
/// Returns the authenticated merchant's BYTS balance.
/// Returns { "amount": 0 } if no balance record exists.
pub async fn get_balance(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<BytsBalanceResponse>, AppError> {
    let amount = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1",
        auth.user_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0);

    Ok(Json(BytsBalanceResponse { amount }))
}

/// POST /api/byts/lock
///
/// Locks BYTS for the authenticated merchant. Deducts from byts_balance and
/// writes a chain event — all inside a single PostgreSQL transaction.
///
/// Returns 422 if the merchant's BYTS balance is insufficient.
pub async fn lock_byts(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<LockRequest>,
) -> Result<Json<LockResponse>, AppError> {
    if body.amount <= 0 {
        return Err(AppError::BadRequest("amount must be positive".to_string()));
    }

    let mut tx = state.pool.begin().await.map_err(AppError::Database)?;

    // Step 1: SELECT FOR UPDATE — acquire row-level lock to prevent concurrent deductions.
    let current_balance = sqlx::query_scalar!(
        "SELECT amount FROM byts_balances WHERE user_id = $1 FOR UPDATE",
        auth.user_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::Database)?
    .unwrap_or(0);

    if current_balance < body.amount {
        tx.rollback().await.map_err(AppError::Database)?;
        return Err(AppError::UnprocessableEntity(
            "insufficient BYTS balance".to_string(),
        ));
    }

    // Step 2: INSERT byts_lock record.
    let lock_id = sqlx::query_scalar!(
        "INSERT INTO byts_locks (user_id, amount) VALUES ($1, $2) RETURNING id",
        auth.user_id,
        body.amount,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 3: Deduct from balance.
    sqlx::query!(
        "UPDATE byts_balances SET amount = amount - $1, updated_at = now() WHERE user_id = $2",
        body.amount,
        auth.user_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 4: Write chain event and block in the same transaction.
    // Payload keys are alphabetical to match PostgreSQL JSONB read-back order.
    let payload = serde_json::json!({
        "amount": body.amount,
        "lock_id": lock_id,
        "user_id": auth.user_id,
    });
    chain::append_block(&mut tx, "lock", &payload).await?;

    tx.commit().await.map_err(AppError::Database)?;

    Ok(Json(LockResponse {
        lock_id,
        amount: body.amount,
        status: "active".to_string(),
    }))
}

/// POST /api/byts/unlock
///
/// Releases an active BYTS lock for the authenticated merchant. Restores the
/// balance and writes a chain event — all inside a single PostgreSQL transaction.
///
/// Returns 422 if the lock does not exist, is not owned by this merchant,
/// or has already been released.
pub async fn unlock_byts(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UnlockRequest>,
) -> Result<Json<UnlockResponse>, AppError> {
    let mut tx = state.pool.begin().await.map_err(AppError::Database)?;

    // Step 1: SELECT FOR UPDATE — verify lock exists, is active, and belongs to this merchant.
    let lock = sqlx::query!(
        r#"SELECT id, amount
           FROM byts_locks
           WHERE id = $1 AND user_id = $2 AND status = 'active'
           FOR UPDATE"#,
        body.lock_id,
        auth.user_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    let lock = match lock {
        Some(l) => l,
        None => {
            tx.rollback().await.map_err(AppError::Database)?;
            return Err(AppError::UnprocessableEntity(
                "lock not found or already released".to_string(),
            ));
        }
    };

    // Step 2: Mark lock as released.
    sqlx::query!(
        "UPDATE byts_locks SET status = 'released', unlocked_at = now() WHERE id = $1",
        lock.id,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 3: Restore balance. UPSERT handles the edge case where balance row was removed.
    sqlx::query!(
        r#"INSERT INTO byts_balances (user_id, amount) VALUES ($1, $2)
           ON CONFLICT (user_id) DO UPDATE
           SET amount = byts_balances.amount + EXCLUDED.amount,
               updated_at = now()"#,
        auth.user_id,
        lock.amount,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 4: Write chain event and block in the same transaction.
    let payload = serde_json::json!({
        "amount": lock.amount,
        "lock_id": lock.id,
        "user_id": auth.user_id,
    });
    chain::append_block(&mut tx, "unlock", &payload).await?;

    tx.commit().await.map_err(AppError::Database)?;

    Ok(Json(UnlockResponse {
        lock_id: lock.id,
        status: "released".to_string(),
    }))
}

/// GET /api/byts/lock
///
/// Returns all BYTS locks (active and released) for the authenticated merchant,
/// ordered by locked_at descending.
pub async fn list_locks(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<LockDetailResponse>>, AppError> {
    let rows = sqlx::query_as!(
        BytsLock,
        r#"SELECT id, user_id, amount, locked_at, unlocked_at, status
           FROM byts_locks
           WHERE user_id = $1
           ORDER BY locked_at DESC"#,
        auth.user_id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    Ok(Json(
        rows.into_iter().map(LockDetailResponse::from).collect(),
    ))
}
