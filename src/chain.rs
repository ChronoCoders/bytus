use sha2::{Digest, Sha256};
use sqlx::PgPool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChainError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("block hash mismatch on block {0}")]
    HashMismatch(i64),
    #[error("block not found: {0}")]
    NotFound(i64),
}

/// Compute SHA-256(prev_hash || payload_0_bytes || payload_1_bytes || ...).
///
/// Each payload is serialized as a UTF-8 JSON string. serde_json uses BTreeMap
/// for JSON objects (alphabetical key order) when the `preserve_order` feature
/// is absent. PostgreSQL JSONB also sorts object keys alphabetically. Both
/// produce the same byte sequence, so hash computed at write time and at verify
/// time are identical.
///
/// Genesis block has no events: hash = SHA-256(prev_hash).
pub fn compute_block_hash(prev_hash: &[u8], event_payloads: &[serde_json::Value]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    for payload in event_payloads {
        hasher.update(payload.to_string().as_bytes());
    }
    hasher.finalize().to_vec()
}

/// Append one block with one chain event to the chain.
///
/// Must be called inside an existing [`sqlx::Transaction`]. The caller commits
/// or rolls back. If the caller rolls back, the block and event roll back too —
/// chain state and financial state are always in sync.
///
/// The latest block row is locked with `FOR UPDATE` before its hash is read.
/// This prevents two concurrent transactions from forking the chain by producing
/// the same `prev_hash`.
pub async fn append_block(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_type: &str,
    payload: &serde_json::Value,
) -> Result<i64, ChainError> {
    let prev = sqlx::query!("SELECT id, hash FROM blocks ORDER BY id DESC LIMIT 1 FOR UPDATE")
        .fetch_one(&mut **tx)
        .await
        .map_err(ChainError::Database)?;

    let new_hash = compute_block_hash(&prev.hash, std::slice::from_ref(payload));

    let row = sqlx::query!(
        "INSERT INTO blocks (prev_hash, hash) VALUES ($1, $2) RETURNING id",
        prev.hash.as_slice(),
        new_hash.as_slice(),
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(ChainError::Database)?;

    sqlx::query!(
        "INSERT INTO chain_events (block_id, event_type, payload) VALUES ($1, $2, $3)",
        row.id,
        event_type,
        payload,
    )
    .execute(&mut **tx)
    .await
    .map_err(ChainError::Database)?;

    Ok(row.id)
}

/// Verify a block by recomputing its hash from stored events and `prev_hash`.
///
/// Returns `Ok(())` if the stored hash matches the recomputed hash.
/// Returns `Err(ChainError::HashMismatch)` if any block content has been tampered with.
/// Returns `Err(ChainError::NotFound)` if `block_id` does not exist.
pub async fn verify_block(block_id: i64, pool: &PgPool) -> Result<(), ChainError> {
    let block = sqlx::query!(
        "SELECT id, prev_hash, hash FROM blocks WHERE id = $1",
        block_id
    )
    .fetch_optional(pool)
    .await
    .map_err(ChainError::Database)?
    .ok_or(ChainError::NotFound(block_id))?;

    let events = sqlx::query!(
        "SELECT payload FROM chain_events WHERE block_id = $1 ORDER BY id ASC",
        block_id
    )
    .fetch_all(pool)
    .await
    .map_err(ChainError::Database)?;

    let payloads: Vec<serde_json::Value> = events.into_iter().map(|e| e.payload).collect();
    let computed = compute_block_hash(&block.prev_hash, &payloads);

    if computed != block.hash {
        return Err(ChainError::HashMismatch(block_id));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    // Pure unit test: output is always 32 bytes and deterministic.
    #[test]
    fn test_genesis_hash_is_deterministic() {
        let h1 = compute_block_hash(&[0u8; 32], &[]);
        let h2 = compute_block_hash(&[0u8; 32], &[]);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 32);
    }

    // The genesis block hash stored by the migration (pgcrypto) must match
    // what the Rust sha2 crate computes from the same input.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_genesis_block_hash_matches_rust(pool: PgPool) {
        let row = sqlx::query!("SELECT prev_hash, hash FROM blocks WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let computed = compute_block_hash(&row.prev_hash, &[]);
        assert_eq!(
            computed, row.hash,
            "genesis hash mismatch between pgcrypto and sha2"
        );
    }

    // Append a block, then pass it through verify_block.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_append_and_verify_block(pool: PgPool) {
        let payload = serde_json::json!({"amount": 1000, "currency": "USD"});

        let mut tx = pool.begin().await.unwrap();
        let block_id = append_block(&mut tx, "settlement", &payload).await.unwrap();
        tx.commit().await.unwrap();

        assert!(
            block_id > 1,
            "new block id must be greater than genesis (1)"
        );
        verify_block(block_id, &pool).await.unwrap();
    }

    // Each new block's prev_hash must equal the previous block's hash.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_chain_linkage(pool: PgPool) {
        let payload = serde_json::json!({"amount": 500, "currency": "USD"});

        let mut tx = pool.begin().await.unwrap();
        let _block_id = append_block(&mut tx, "settlement", &payload).await.unwrap();
        tx.commit().await.unwrap();

        let blocks = sqlx::query!("SELECT prev_hash, hash FROM blocks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(blocks.len(), 2, "expected genesis + 1 appended block");
        assert_eq!(
            blocks[1].prev_hash, blocks[0].hash,
            "new block prev_hash must equal previous block hash"
        );
    }

    // Overwrite a block's stored hash and confirm verify_block detects the tamper.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_tamper_detection(pool: PgPool) {
        let payload = serde_json::json!({"amount": 999, "currency": "USD"});

        let mut tx = pool.begin().await.unwrap();
        let block_id = append_block(&mut tx, "settlement", &payload).await.unwrap();
        tx.commit().await.unwrap();

        // Simulate DB tampering by zeroing out the stored hash.
        sqlx::query!(
            "UPDATE blocks SET hash = $1 WHERE id = $2",
            &[0u8; 32][..],
            block_id,
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = verify_block(block_id, &pool).await;
        assert!(
            matches!(result, Err(ChainError::HashMismatch(_))),
            "expected HashMismatch after tamper, got {result:?}"
        );
    }
}
