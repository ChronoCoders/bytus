use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::chain;
use crate::error::AppError;
use crate::models::Settlement;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateSettlementRequest {
    pub gross_amount: i64,
    pub idempotency_key: String,
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String {
    "USD".to_string()
}

#[derive(Serialize)]
pub struct SettlementResponse {
    pub id: Uuid,
    pub user_id: Uuid,
    pub gross_amount: i64,
    pub fee_amount: i64,
    pub net_amount: i64,
    pub byts_fee: i64,
    pub currency: String,
    pub status: String,
    pub idempotency_key: String,
    pub created_at: DateTime<Utc>,
}

impl From<Settlement> for SettlementResponse {
    fn from(s: Settlement) -> Self {
        Self {
            id: s.id,
            user_id: s.user_id,
            gross_amount: s.gross_amount,
            fee_amount: s.fee_amount,
            net_amount: s.net_amount,
            byts_fee: s.byts_fee,
            currency: s.currency,
            status: s.status,
            idempotency_key: s.idempotency_key,
            created_at: s.created_at,
        }
    }
}

pub async fn create_settlement(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateSettlementRequest>,
) -> Result<Json<SettlementResponse>, AppError> {
    if body.gross_amount <= 0 {
        return Err(AppError::BadRequest(
            "gross_amount must be positive".to_string(),
        ));
    }

    let gross: i64 = body.gross_amount;
    let fee: i64 = gross * i64::from(state.fee_bps) / 10_000;
    let net: i64 = gross - fee;

    if net <= 0 {
        return Err(AppError::BadRequest(
            "net amount must be positive".to_string(),
        ));
    }

    // BYTS fee: fiat_fee * byts_rate / 1_000_000 (integer division, rate in micro-units).
    let byts_fee: i64 = fee * i64::from(state.byts_rate) / 1_000_000;

    let mut tx = state.pool.begin().await.map_err(AppError::Database)?;

    // Step 1: INSERT settlement — ON CONFLICT DO NOTHING returns None on duplicate key.
    let settlement_opt = sqlx::query_as!(
        Settlement,
        r#"INSERT INTO settlements (user_id, gross_amount, fee_amount, net_amount, byts_fee, currency, idempotency_key)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (idempotency_key) DO NOTHING
           RETURNING id, user_id, gross_amount, fee_amount, net_amount, byts_fee, currency, status, idempotency_key, created_at"#,
        auth.user_id,
        gross,
        fee,
        net,
        byts_fee,
        body.currency,
        body.idempotency_key,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    let settlement = match settlement_opt {
        Some(s) => s,
        None => {
            // Idempotency key already exists — return existing, no further writes.
            tx.rollback().await.map_err(AppError::Database)?;
            let existing = sqlx::query_as!(
                Settlement,
                r#"SELECT id, user_id, gross_amount, fee_amount, net_amount, byts_fee, currency, status, idempotency_key, created_at
                   FROM settlements
                   WHERE idempotency_key = $1 AND user_id = $2"#,
                body.idempotency_key,
                auth.user_id,
            )
            .fetch_one(&state.pool)
            .await
            .map_err(AppError::Database)?;
            return Ok(Json(SettlementResponse::from(existing)));
        }
    };

    // Step 2: INSERT ledger credit entry.
    sqlx::query!(
        "INSERT INTO ledger_entries (user_id, settlement_id, entry_type, amount)
         VALUES ($1, $2, 'credit', $3)",
        auth.user_id,
        settlement.id,
        gross,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 3: INSERT ledger fee entry (amount is negative).
    let fee_entry_amount: i64 = -fee;
    sqlx::query!(
        "INSERT INTO ledger_entries (user_id, settlement_id, entry_type, amount)
         VALUES ($1, $2, 'fee', $3)",
        auth.user_id,
        settlement.id,
        fee_entry_amount,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 4: INSERT apply_marker — UNIQUE on settlement_id.
    // Returns Some(id) if newly inserted, None if already present (concurrent retry).
    let marker_inserted = sqlx::query_scalar!(
        "INSERT INTO apply_markers (settlement_id)
         VALUES ($1)
         ON CONFLICT (settlement_id) DO NOTHING
         RETURNING id",
        settlement.id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    // Step 5: Update balance only if the apply_marker was just inserted (exactly-once).
    if marker_inserted.is_some() {
        sqlx::query!(
            r#"INSERT INTO balances (user_id, currency, amount)
               VALUES ($1, $2, $3)
               ON CONFLICT (user_id, currency) DO UPDATE
               SET amount = balances.amount + EXCLUDED.amount,
                   updated_at = now()"#,
            auth.user_id,
            body.currency,
            net,
        )
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    }

    // Step 6: Write chain event and block in the same transaction.
    // Idempotent replay returns early before this point — no duplicate events.
    // Payload keys are alphabetical to match PostgreSQL JSONB read-back order.
    let payload = serde_json::json!({
        "currency": body.currency,
        "fee_amount": fee,
        "gross_amount": gross,
        "net_amount": net,
        "settlement_id": settlement.id,
    });
    chain::append_block(&mut tx, "settlement", &payload).await?;

    tx.commit().await.map_err(AppError::Database)?;

    Ok(Json(SettlementResponse::from(settlement)))
}

pub async fn list_settlements(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<SettlementResponse>>, AppError> {
    let rows = sqlx::query_as!(
        Settlement,
        r#"SELECT id, user_id, gross_amount, fee_amount, net_amount, byts_fee, currency, status, idempotency_key, created_at
           FROM settlements
           WHERE user_id = $1
           ORDER BY created_at DESC"#,
        auth.user_id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    Ok(Json(
        rows.into_iter().map(SettlementResponse::from).collect(),
    ))
}
