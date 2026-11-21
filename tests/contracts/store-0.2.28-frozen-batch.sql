INSERT INTO run_notices (run_id, chat_session_id, kind, dedupe, body, created_at, client_turn_id)
VALUES
    ('discarded', 'partial', 'terminal', 'terminal', 'Discarded run finished', 10, 'run-notice:frozen'),
    ('surviving', 'partial', 'terminal', 'terminal', 'Surviving run finished', 11, 'run-notice:frozen'),
    ('older-time', 'partial', 'terminal', 'terminal', 'Run that finished first', 9, 'run-notice:later-id');
