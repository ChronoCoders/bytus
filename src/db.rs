use sqlx::PgPool;

use crate::error::AppError;

pub async fn connect(database_url: &str) -> Result<PgPool, AppError> {
    PgPool::connect(database_url)
        .await
        .map_err(AppError::Database)
}
