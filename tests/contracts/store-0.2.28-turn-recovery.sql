-- v0.2.27 and v0.2.28 load this turn for Retry and Continue.
SELECT id, session_id, user_message_id, assistant_message_id,
    client_turn_id, request_hash, prepared_input, settings_json, state, delivery_state,
    attempt_count, next_retry_at, error_kind, error_message, recovery_action,
    recovered_by_turn_id, created_at, updated_at
FROM chat_turns WHERE id = ?1 AND session_id = ?2;
