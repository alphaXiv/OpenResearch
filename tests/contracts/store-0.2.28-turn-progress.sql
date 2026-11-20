UPDATE chat_turns SET state = ?2, delivery_state = ?3, attempt_count = ?4,
    next_retry_at = ?5, updated_at = ?6
WHERE id = ?1 AND state IN ('preparing', 'retrying', 'running');
