CREATE TABLE IF NOT EXISTS chat_turns (
    id                   TEXT PRIMARY KEY,
    session_id           TEXT NOT NULL,
    user_message_id      TEXT,
    assistant_message_id TEXT NOT NULL,
    client_turn_id       TEXT NOT NULL,
    request_hash         TEXT NOT NULL,
    prepared_input       TEXT NOT NULL,
    settings_json        TEXT NOT NULL,
    state                TEXT NOT NULL,
    delivery_state       TEXT NOT NULL,
    attempt_count        INTEGER NOT NULL DEFAULT 0,
    next_retry_at        INTEGER,
    error_kind           TEXT,
    error_message        TEXT,
    recovery_action      TEXT,
    recovered_by_turn_id TEXT,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    UNIQUE(session_id, client_turn_id)
);
INSERT INTO chat_turns (
    id, session_id, assistant_message_id, client_turn_id, request_hash,
    prepared_input, settings_json, state, delivery_state, created_at, updated_at
) VALUES
    ('old-turn', 'old-chat', 'old-answer', 'old-client', 'hash',
     'question', '{}', 'running', 'accepted', 1, 1),
    ('old-source', 'old-chat', 'old-source-answer', 'old-source-client', 'hash',
     'question', '{}', 'running', 'accepted', 2, 2);
