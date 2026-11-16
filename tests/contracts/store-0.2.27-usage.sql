-- The usage ledger and runs table as orx 0.2.27 creates them: a codex turn's sample, which
-- carries no time or cost, and a finished Hugging Face run, which has no price.
CREATE TABLE chat_usage_executions (
                execution_id TEXT PRIMARY KEY,
                turn_id TEXT NOT NULL,
                harness TEXT NOT NULL,
                report_id TEXT NOT NULL,
                suppressed INTEGER NOT NULL,
                outcome TEXT
            );
CREATE TABLE chat_usage_samples (
                execution_id TEXT NOT NULL,
                sample_id TEXT NOT NULL,
                harness TEXT NOT NULL,
                model TEXT,
                provider TEXT,
                usage_json TEXT NOT NULL,
                complete INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (execution_id, sample_id)
            );
CREATE TABLE runs (
                id           TEXT PRIMARY KEY,
                experiment_id TEXT NOT NULL,
                project_id   TEXT NOT NULL,
                status       TEXT NOT NULL,
                backend_json TEXT NOT NULL,
                command      TEXT NOT NULL DEFAULT '',
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL,
                ended_at     INTEGER,
                exit_code    INTEGER
            , commit_sha TEXT, result_markdown TEXT, cancel_requested INTEGER NOT NULL DEFAULT 0, chat_session_id TEXT, cancel_reason TEXT, cancel_unconfirmed TEXT, contact TEXT, spec_digest TEXT, submission_attempt INTEGER NOT NULL DEFAULT 1, submission_fence TEXT, cancel_accepted_at INTEGER, launch_spec TEXT, notify_cursor TEXT, started_at INTEGER, waiting_reason TEXT);
INSERT INTO chat_usage_executions VALUES ('codex-turn','turn_1','codex','report','1','done');
INSERT INTO chat_usage_samples VALUES ('codex-turn','thread:turn:0:{}','codex','gpt-5.5',NULL,'{"inputTokens":1000,"outputTokens":100,"cacheReadTokens":800,"cacheWriteTokens":0,"reasoningTokens":20}',1);
INSERT INTO runs VALUES ('hf','exp_1','proj_1','done','{"kind":"engine","jobId":"hf-1","handle":{"type":"Api","provider":"hf_job","account":{"target":"lab","context":null},"ids":["hf-1"]}}','python train.py',1794314000000,1794318000000,1794318000000,0,NULL,NULL,0,'chat_1',NULL,NULL,NULL,'digest',1,NULL,NULL,'{"kind":"hf_job","target":"lab","options":{"flavor":"a10g-small"}}',NULL,1794314400000,NULL);
