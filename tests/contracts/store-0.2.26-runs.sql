-- The runs table as orx 0.2.26 creates it, with engine runs it wrote: one starting, one running.
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
            , commit_sha TEXT, result_markdown TEXT, cancel_requested INTEGER NOT NULL DEFAULT 0, chat_session_id TEXT, cancel_reason TEXT, cancel_unconfirmed TEXT, contact TEXT, spec_digest TEXT, submission_attempt INTEGER NOT NULL DEFAULT 1, submission_fence TEXT, cancel_accepted_at INTEGER, launch_spec TEXT, notify_cursor TEXT, started_at INTEGER);
INSERT INTO runs VALUES ('starting','exp_1','proj_1','starting','{"kind":"engine","jobId":"job/orx-starting","sourceDigest":"abc","handle":{"type":"Api","provider":"k8s_job","account":{"target":"experiments","context":"research"},"ids":["job/orx-starting"]}}','python train.py',100,200,NULL,NULL,'0123456789abcdef0123456789abcdef01234567',NULL,0,'chat_1',NULL,NULL,NULL,'digest',1,'{"taken_at":150,"submitter":null}',NULL,'{"kind":"k8s_job","target":"experiments","options":{"context":"research"}}',NULL,NULL);
INSERT INTO runs VALUES ('running','exp_1','proj_1','running','{"kind":"engine","jobId":"job/orx-running","sourceDigest":"abc","handle":{"type":"Api","provider":"k8s_job","account":{"target":"experiments","context":"research"},"ids":["job/orx-running"]}}','python train.py',100,300,NULL,NULL,NULL,NULL,0,'chat_1',NULL,NULL,NULL,'digest',1,'{"taken_at":150,"submitter":null}',NULL,'{"kind":"k8s_job","target":"experiments","options":{"context":"research"}}',NULL,250);
