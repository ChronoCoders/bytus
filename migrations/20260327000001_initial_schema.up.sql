CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        NOT NULL UNIQUE,
    password_hash TEXT        NOT NULL,
    kyc_status    TEXT        NOT NULL DEFAULT 'none',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE bus_locks (
    id        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id   UUID        NOT NULL REFERENCES users(id),
    amount    BIGINT      NOT NULL,
    locked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE settlements (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID        NOT NULL REFERENCES users(id),
    gross_amount    BIGINT      NOT NULL,
    fee_amount      BIGINT      NOT NULL,
    net_amount      BIGINT      NOT NULL,
    currency        TEXT        NOT NULL DEFAULT 'USD',
    status          TEXT        NOT NULL DEFAULT 'completed',
    idempotency_key TEXT        NOT NULL UNIQUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE ledger_entries (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID        NOT NULL REFERENCES users(id),
    settlement_id UUID        NOT NULL REFERENCES settlements(id),
    entry_type    TEXT        NOT NULL,
    amount        BIGINT      NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE balances (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL,
    currency   TEXT        NOT NULL DEFAULT 'USD',
    amount     BIGINT      NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, currency)
);

CREATE TABLE apply_markers (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    settlement_id UUID        NOT NULL REFERENCES settlements(id) UNIQUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
