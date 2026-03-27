pub mod auth;
pub mod me;
pub mod settlements;

use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/auth/signup", post(auth::signup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/me", get(me::me))
        .route("/api/bus/lock", get(bus_lock_get).post(bus_lock_post))
        .route(
            "/api/settlements",
            get(settlements::list_settlements).post(settlements::create_settlement),
        )
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn bus_lock_get(_auth: AuthUser) -> Result<StatusCode, AppError> {
    Ok(StatusCode::NOT_IMPLEMENTED)
}

async fn bus_lock_post(_auth: AuthUser) -> Result<StatusCode, AppError> {
    Ok(StatusCode::NOT_IMPLEMENTED)
}
