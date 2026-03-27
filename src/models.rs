use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct BytsLock {
    pub id: Uuid,
    pub user_id: Uuid,
    pub amount: i64,
    pub locked_at: DateTime<Utc>,
    pub unlocked_at: Option<DateTime<Utc>>,
    pub status: String,
}

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub kyc_status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
pub struct Settlement {
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
