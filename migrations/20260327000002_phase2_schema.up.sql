CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Add BYTS fee column to settlements (backward-compatible, defaults to 0).
ALTER TABLE settlements ADD COLUMN byts_fee BIGINT NOT NULL DEFAULT 0;

-- Append-only chain: one block per financial operation.
-- BIGINT GENERATED ALWAYS AS IDENTITY gives deterministic insertion order.
CREATE TABLE blocks (
    id         BIGINT      PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    prev_hash  BYTEA       NOT NULL,
    hash       BYTEA       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE chain_events (
    id         BIGINT      PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    block_id   BIGINT      NOT NULL REFERENCES blocks(id),
    event_type TEXT        NOT NULL,
    payload    JSONB       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- BYTS internal token tables.
CREATE TABLE byts_balances (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id),
    amount     BIGINT      NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id)
);

CREATE TABLE byts_locks (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users(id),
    amount      BIGINT      NOT NULL,
    locked_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    unlocked_at TIMESTAMPTZ,
    status      TEXT        NOT NULL DEFAULT 'active'
);

-- Genesis block: id = 1, prev_hash = 32 zero bytes.
-- hash = SHA-256(prev_hash) with no events — computed by pgcrypto so it
-- matches exactly what the Rust sha2 crate produces.
INSERT INTO blocks (prev_hash, hash)
VALUES (
    '\x0000000000000000000000000000000000000000000000000000000000000000'::bytea,
    digest('\x0000000000000000000000000000000000000000000000000000000000000000'::bytea, 'sha256')
);
