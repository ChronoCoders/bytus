use crate::handlers::auth::Claims;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct TransactionQuery {
    pub search: Option<String>,
    pub filter: Option<String>,
    pub page: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Serialize)]
pub struct Transaction {
    pub id: String,
    pub tx_type: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
    pub created_at: String,
    pub customer_email: Option<String>,
}

#[derive(Serialize)]
pub struct TransactionListResponse {
    pub transactions: Vec<Transaction>,
    pub total: i32,
    pub page: i32,
}

#[derive(Serialize)]
pub struct TransactionDetail {
    pub id: String,
    pub tx_type: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
    pub created_at: String,
    pub customer_email: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn list_transactions(
    State(pool): State<PgPool>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<TransactionQuery>,
) -> Result<Json<TransactionListResponse>, StatusCode> {
    let page = params.page.unwrap_or(1);
    let limit = params.limit.unwrap_or(10);
    let offset = (page - 1) * limit;

    let mut query = String::from(
        "SELECT id, tx_type, amount, currency, status, customer_email, created_at FROM transactions WHERE 1=1"
    );

    if let Some(search) = &params.search {
        query.push_str(&format!(" AND (customer_email ILIKE '%{}%' OR status ILIKE '%{}%')", search, search));
    }

    if let Some(filter) = &params.filter {
        query.push_str(&format!(" AND status = '{}'", filter));
    }

    query.push_str(" ORDER BY created_at DESC");
    query.push_str(&format!(" LIMIT {} OFFSET {}", limit, offset));

    let rows = sqlx::query_as::<_, (Uuid, String, bigdecimal::BigDecimal, String, String, Option<String>, chrono::NaiveDateTime)>(&query)
        .fetch_all(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let transactions: Vec<Transaction> = rows
        .into_iter()
        .map(|(id, tx_type, amount, currency, status, customer_email, created_at)| Transaction {
            id: id.to_string(),
            tx_type,
            amount: amount.to_string(),
            currency,
            status,
            created_at: created_at.to_string(),
            customer_email,
        })
        .collect();

    let total_query = "SELECT COUNT(*) FROM transactions";
    let total: i64 = sqlx::query_scalar(total_query)
        .fetch_one(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(TransactionListResponse {
        transactions,
        total: total as i32,
        page,
    }))
}

pub async fn get_transaction(
    State(pool): State<PgPool>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<TransactionDetail>, StatusCode> {
    let result = sqlx::query!(
        r#"
        SELECT id, tx_type, amount, currency, status, customer_email, metadata, created_at
        FROM transactions
        WHERE id = $1
        "#,
        id
    )
    .fetch_one(&pool)
    .await
    .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(TransactionDetail {
        id: result.id.to_string(),
        tx_type: result.tx_type,
        amount: result.amount.to_string(),
        currency: result.currency,
        status: result.status,
        created_at: result.created_at.unwrap().to_string(),
        customer_email: result.customer_email,
        metadata: result.metadata,
    }))
}