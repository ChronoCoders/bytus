use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{encode_token, hash_password, verify_password};
use crate::error::AppError;
use crate::models::User;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct AuthRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub user_id: Uuid,
    pub token: String,
}

pub async fn signup(
    State(state): State<AppState>,
    Json(body): Json<AuthRequest>,
) -> Result<(StatusCode, Json<AuthResponse>), AppError> {
    if body.email.is_empty() || body.password.is_empty() {
        return Err(AppError::BadRequest(
            "email and password are required".to_string(),
        ));
    }

    let password_hash = hash_password(&body.password)?;

    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (email, password_hash) VALUES ($1, $2) RETURNING id",
        body.email,
        password_hash,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db_err) if db_err.constraint() == Some("users_email_key") => {
            AppError::Conflict("email already registered".to_string())
        }
        _ => AppError::Database(e),
    })?;

    let token = encode_token(user_id, &state.jwt_secret)?;
    Ok((StatusCode::CREATED, Json(AuthResponse { user_id, token })))
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let user = sqlx::query_as!(
        User,
        "SELECT id, email, password_hash, kyc_status, created_at
         FROM users WHERE email = $1",
        body.email,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or(AppError::Unauthorized)?;

    verify_password(&body.password, &user.password_hash)?;

    let token = encode_token(user.id, &state.jwt_secret)?;
    Ok(Json(AuthResponse {
        user_id: user.id,
        token,
    }))
}
