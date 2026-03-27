use axum::{extract::State, Json};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct BytsBalanceResponse {
    pub amount: i64,
}

/// GET /api/byts/balance
///
/// Returns the authenticated merchant's BYTS balance.
/// Returns { "amount": 0 } if no balance record has been created yet.
/// Initial BYTS supply is funded via direct DB seed (operator or test setup).
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
