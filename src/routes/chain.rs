use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

// ── response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct BlockSummaryResponse {
    pub id: i64,
    pub hash: String,
    pub prev_hash: String,
    pub created_at: DateTime<Utc>,
    pub event_count: i64,
}

#[derive(Serialize)]
pub struct EventResponse {
    pub id: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct BlockDetailResponse {
    pub id: i64,
    pub hash: String,
    pub prev_hash: String,
    pub created_at: DateTime<Utc>,
    pub events: Vec<EventResponse>,
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// GET /api/chain/blocks
///
/// Returns the 50 most recent blocks, ordered newest first.
/// Each entry includes a pre-computed event count.
/// Authentication required; chain is a global ledger, not filtered by merchant.
pub async fn list_blocks(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<BlockSummaryResponse>>, AppError> {
    let rows = sqlx::query!(
        r#"SELECT
               b.id,
               b.hash,
               b.prev_hash,
               b.created_at,
               (SELECT COUNT(*) FROM chain_events WHERE block_id = b.id) AS event_count
           FROM blocks b
           ORDER BY b.id DESC
           LIMIT 50"#
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let result = rows
        .into_iter()
        .map(|r| BlockSummaryResponse {
            id: r.id,
            hash: to_hex(&r.hash),
            prev_hash: to_hex(&r.prev_hash),
            created_at: r.created_at,
            event_count: r.event_count.unwrap_or(0),
        })
        .collect();

    Ok(Json(result))
}

/// GET /api/chain/blocks/:id
///
/// Returns a single block with all its chain events ordered by insertion id.
/// Returns 404 if the block does not exist.
pub async fn get_block(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(block_id): Path<i64>,
) -> Result<Json<BlockDetailResponse>, AppError> {
    let block = sqlx::query!(
        "SELECT id, hash, prev_hash, created_at FROM blocks WHERE id = $1",
        block_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or(AppError::NotFound)?;

    let events = sqlx::query!(
        r#"SELECT id, event_type, payload, created_at
           FROM chain_events
           WHERE block_id = $1
           ORDER BY id ASC"#,
        block_id,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let event_responses = events
        .into_iter()
        .map(|e| EventResponse {
            id: e.id,
            event_type: e.event_type,
            payload: e.payload,
            created_at: e.created_at,
        })
        .collect();

    Ok(Json(BlockDetailResponse {
        id: block.id,
        hash: to_hex(&block.hash),
        prev_hash: to_hex(&block.prev_hash),
        created_at: block.created_at,
        events: event_responses,
    }))
}
