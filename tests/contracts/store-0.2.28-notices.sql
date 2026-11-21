-- v0.2.28's outbox table with a claimed result in retry backoff.
CREATE TABLE run_notices (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL,
    chat_session_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    dedupe TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    client_turn_id TEXT,
    delivered_at INTEGER,
    delivery_failures INTEGER NOT NULL DEFAULT 0,
    first_failure_at INTEGER,
    next_delivery_at INTEGER,
    delivery_error TEXT,
    retired_at INTEGER,
    UNIQUE(run_id, chat_session_id, dedupe)
);
INSERT INTO run_notices (run_id, chat_session_id, kind, dedupe, body, created_at,
    client_turn_id, delivery_failures, first_failure_at, next_delivery_at, delivery_error)
VALUES ('released-run', 'owner', 'terminal', 'terminal', 'Run finished', 1,
    'run-notice:00000000-0000-4000-8000-000000000028', 1, 2, 60002, 'Harness unavailable');
