-- v0.2.28 Store::pending_run_notice_sessions, the chat outbox drain's first query.
SELECT DISTINCT chat_session_id FROM run_notices
WHERE delivered_at IS NULL AND retired_at IS NULL
AND (next_delivery_at IS NULL OR next_delivery_at <= ?1) ORDER BY chat_session_id;
