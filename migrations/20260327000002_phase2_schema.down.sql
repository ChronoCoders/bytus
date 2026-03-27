DELETE FROM blocks WHERE id = 1;

DROP TABLE IF EXISTS byts_locks;
DROP TABLE IF EXISTS byts_balances;
DROP TABLE IF EXISTS chain_events;
DROP TABLE IF EXISTS blocks;

ALTER TABLE settlements DROP COLUMN IF EXISTS byts_fee;
