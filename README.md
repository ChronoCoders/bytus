# Bytus

B2B crypto-to-fiat settlement infrastructure. Accepts payment inputs, records balances in an internal ledger, and settles funds to merchant accounts.

PostgreSQL is the single source of truth for all financial state. No blockchain. No external integrations. Correct accounting only.

## Stack

- **Rust** (Axum 0.7)
- **PostgreSQL 16** — authoritative ledger
- **SQLx 0.8** — compile-time verified queries
- **Argon2id** — password hashing
- **JWT** (HS256) — authentication

## Prerequisites

- Rust (stable)
- PostgreSQL 16
- [sqlx-cli](https://github.com/launchbadge/sqlx/tree/main/sqlx-cli)

```
cargo install sqlx-cli --no-default-features --features rustls,postgres
```

## Setup

**1. Clone and configure**

```bash
git clone https://github.com/ChronoCoders/bytus.git
cd bytus
cp .env.example .env
```

Edit `.env`:

```env
DATABASE_URL=postgres://user:password@localhost:5432/bytus
JWT_SECRET=your_secret_minimum_32_characters_long
FEE_BPS=50
```

`FEE_BPS` is the platform fee in basis points (50 = 0.5%).

**2. Create the database**

```bash
psql -U postgres -c "CREATE DATABASE bytus;"
```

**3. Run migrations**

```bash
sqlx migrate run
```

**4. Build and run**

```bash
cargo build --release
./target/release/bytus
```

Server binds to `0.0.0.0:8080`.

## API

All endpoints except `/health` require `Authorization: Bearer <token>`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `POST` | `/api/auth/signup` | Register a merchant account |
| `POST` | `/api/auth/login` | Authenticate and receive a JWT |
| `GET` | `/api/me` | Current authenticated user |
| `POST` | `/api/settlements` | Create a settlement |
| `GET` | `/api/settlements` | List settlements for the authenticated merchant |
| `GET` | `/api/bus/lock` | Placeholder |
| `POST` | `/api/bus/lock` | Placeholder |

### Signup

```bash
curl -X POST http://localhost:8080/api/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email": "merchant@example.com", "password": "securepassword"}'
```

```json
{
  "user_id": "2b1d4c5e-...",
  "token": "eyJ0eXAi..."
}
```

### Login

```bash
curl -X POST http://localhost:8080/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "merchant@example.com", "password": "securepassword"}'
```

### Create Settlement

```bash
curl -X POST http://localhost:8080/api/settlements \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "gross_amount": 10000,
    "currency": "USD",
    "idempotency_key": "order-abc-001"
  }'
```

```json
{
  "id": "6c3ccdfb-...",
  "user_id": "2b1d4c5e-...",
  "gross_amount": 10000,
  "fee_amount": 50,
  "net_amount": 9950,
  "currency": "USD",
  "status": "completed",
  "idempotency_key": "order-abc-001",
  "created_at": "2026-03-27T18:34:51Z"
}
```

Amounts are in minor currency units (cents). `idempotency_key` guarantees exactly-once processing — submitting the same key twice returns the original settlement without writing new records.

### Fee Calculation

```
fee = gross_amount * FEE_BPS / 10000
net = gross_amount - fee
```

Integer arithmetic only. No floating point.

## Settlement Pipeline

Every `POST /api/settlements` executes atomically inside a single PostgreSQL transaction:

```
BEGIN
  INSERT settlement          (idempotency_key UNIQUE — duplicate returns existing)
  INSERT ledger credit entry (type = 'credit', amount = gross_amount)
  INSERT ledger fee entry    (type = 'fee',    amount = -fee_amount)
  INSERT apply_marker        (settlement_id UNIQUE — exactly-once guard)
  IF apply_marker inserted:
    UPSERT balance           (amount += net_amount)
COMMIT
```

Partial state is a bug. Retries are safe.

## Database Schema

| Table | Purpose |
|-------|---------|
| `users` | Merchant accounts |
| `settlements` | Settlement records |
| `ledger_entries` | Double-entry ledger (credit + fee per settlement) |
| `balances` | Running merchant balance per currency |
| `apply_markers` | Exactly-once guard for balance updates |
| `bus_locks` | Reserved for Phase 2 |

All monetary values are `BIGINT` (minor units). No `NUMERIC` or `FLOAT`.

## Tests

```bash
cargo test
```

Requires `DATABASE_URL` to be set. Each test runs against an isolated database created and torn down automatically by `sqlx::test`.

```
running 12 tests
test test_health ... ok
test test_signup_returns_user_id_and_token ... ok
test test_signup_duplicate_email_returns_conflict ... ok
test test_login_returns_token ... ok
test test_login_wrong_password_returns_unauthorized ... ok
test test_me_without_token_returns_unauthorized ... ok
test test_me_with_token_returns_user ... ok
test test_settlement_requires_auth ... ok
test test_settlement_happy_path ... ok
test test_settlement_idempotency ... ok
test test_settlement_concurrent_idempotency ... ok
test test_settlement_balance_accuracy ... ok
```

## Offline Builds

The `.sqlx/` directory contains compile-time query metadata. To build without a running database:

```bash
SQLX_OFFLINE=true cargo build
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `JWT_SECRET` | HMAC-SHA256 signing secret (minimum 32 characters) |
| `FEE_BPS` | Platform fee in basis points (e.g. `50` = 0.5%) |
