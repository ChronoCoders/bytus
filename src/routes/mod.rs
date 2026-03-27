pub mod auth;
pub mod byts;
pub mod chain;
pub mod me;
pub mod settlements;

use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::state::AppState;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/auth/signup", post(auth::signup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/me", get(me::me))
        .route("/api/byts/balance", get(byts::get_balance))
        .route(
            "/api/byts/lock",
            get(byts::list_locks).post(byts::lock_byts),
        )
        .route("/api/byts/unlock", post(byts::unlock_byts))
        .route(
            "/api/settlements",
            get(settlements::list_settlements).post(settlements::create_settlement),
        )
        .route("/api/chain/blocks", get(chain::list_blocks))
        .route("/api/chain/blocks/:id", get(chain::get_block))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}
