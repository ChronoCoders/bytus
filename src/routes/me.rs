use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

#[derive(Serialize)]
pub struct MeResponse {
    pub id: Uuid,
    pub email: String,
    pub kyc_status: String,
    pub created_at: DateTime<Utc>,
}

pub async fn me(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MeResponse>, AppError> {
    let user = sqlx::query_as!(
        User,
        "SELECT id, email, password_hash, kyc_status, created_at
         FROM users WHERE id = $1",
        auth.user_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or(AppError::Unauthorized)?;

    Ok(Json(MeResponse {
        id: user.id,
        email: user.email,
        kyc_status: user.kyc_status,
        created_at: user.created_at,
    }))
}
