-- v0.2.28 recovers this fixture's crash after the old server lease expires.
DELETE FROM chat_turn_leases;
UPDATE chat_turns
SET state = 'failed', error_kind = 'server_restarted',
    error_message = 'ORX restarted before this turn finished',
    recovery_action = CASE
        WHEN delivery_state IN ('not_sent', 'rejected') THEN 'retry'
        ELSE 'continue'
    END,
    next_retry_at = NULL, updated_at = 2
WHERE state IN ('preparing', 'retrying', 'running')
  AND NOT EXISTS (
      SELECT 1 FROM chat_turn_leases
      WHERE chat_turn_leases.chat_session_id = chat_turns.session_id
  );

-- The user retries the same turn.
UPDATE chat_turns SET state = 'preparing', delivery_state = 'not_sent',
    attempt_count = 0, next_retry_at = NULL, error_kind = NULL,
    error_message = NULL, recovery_action = NULL, updated_at = 3
WHERE client_turn_id = 'cut-user-turn' AND state = 'failed'
  AND recovery_action = 'retry' AND recovered_by_turn_id IS NULL;
