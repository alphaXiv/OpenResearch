-- Written by Store::open_at/upsert_run and the wakeup writers from orx 0.2.22.
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
            , commit_sha TEXT, result_markdown TEXT, cancel_requested INTEGER NOT NULL DEFAULT 0, chat_session_id TEXT, cancel_reason TEXT);
INSERT INTO runs VALUES ('cancelled','exp_1','proj_1','cancelled','{"kind":"ssh_job","jobId":"finished"}','echo hi',1,1,300,0,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('done','exp_1','proj_1','done','{"kind":"ssh_job","jobId":"finished"}','echo hi',1,1,300,0,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('failed','exp_1','proj_1','failed','{"kind":"ssh_job","jobId":"finished"}','echo hi',1,1,300,0,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('hf_job','exp_1','proj_1','running','{"kind":"hf_job","namespace":"lab","jobId":"handle-4","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000004","sourcePath":"/sources/4.tar","sourceSize":123,"rentalLease":null}','echo hi',104,204,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('k8s_job','exp_1','proj_1','running','{"kind":"k8s_job","namespace":"lab","jobId":"handle-6","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000006","sourcePath":"/sources/6.tar","sourceSize":123,"rentalLease":null}','echo hi',106,206,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('local_job','exp_1','proj_1','running','{"kind":"local_job","namespace":"lab","jobId":"handle-2","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000002","sourcePath":"/sources/2.tar","sourceSize":123,"rentalLease":null}','echo hi',102,202,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('lost','exp_1','proj_1','lost','{"kind":"ssh_job","jobId":"finished"}','echo hi',1,1,300,0,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('modal_job','exp_1','proj_1','starting','{"kind":"modal_job","namespace":"lab","jobId":"handle-5","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000005","sourcePath":"/sources/5.tar","sourceSize":123,"rentalLease":null}','echo hi',105,205,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('openresearch_job','exp_1','proj_1','running','{"kind":"openresearch_job","namespace":"lab","jobId":"handle-8","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000008","sourcePath":"/sources/8.tar","sourceSize":123,"rentalLease":null}','echo hi',108,208,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('ray_job','exp_1','proj_1','starting','{"kind":"ray_job","namespace":"lab","jobId":"handle-7","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000007","sourcePath":"/sources/7.tar","sourceSize":123,"rentalLease":null}','echo hi',107,207,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('slurm_job','exp_1','proj_1','starting','{"kind":"slurm_job","namespace":"lab","jobId":"handle-1","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000001","sourcePath":"/sources/1.tar","sourceSize":123,"rentalLease":null}','echo hi',101,201,NULL,NULL,NULL,NULL,0,'owner',NULL);
INSERT INTO runs VALUES ('ssh_job','exp_1','proj_1','running','{"kind":"ssh_job","namespace":"lab","jobId":"handle-0","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000000","sourcePath":"/sources/0.tar","sourceSize":123,"rentalLease":null}','echo hi',100,200,NULL,NULL,NULL,NULL,1,'owner',NULL);
INSERT INTO runs VALUES ('tinker_job','exp_1','proj_1','starting','{"kind":"tinker_job","namespace":"lab","jobId":"handle-3","sourceDigest":"0000000000000000000000000000000000000000000000000000000000000003","sourcePath":"/sources/3.tar","sourceSize":123,"rentalLease":null}','echo hi',103,203,NULL,NULL,NULL,NULL,0,'owner',NULL);
CREATE TABLE chat_sessions (
                id                TEXT PRIMARY KEY,
                project_id        TEXT NOT NULL,
                harness           TEXT NOT NULL,
                native_session_id TEXT,
                title             TEXT,
                title_source      TEXT,
                model             TEXT,
                service_tier      TEXT,
                permission_mode   TEXT,
                plan_mode         INTEGER NOT NULL DEFAULT 0,
                plan_reset_pending INTEGER NOT NULL DEFAULT 0,
                reasoning_level   TEXT,
                archived          INTEGER NOT NULL DEFAULT 0,
                context_usage_json TEXT,
                bootstrap_context TEXT,
                parent_session_id TEXT,
                created_at        INTEGER NOT NULL,
                updated_at        INTEGER NOT NULL
            , active_leaf_id TEXT, goal TEXT, autonomy TEXT, side_parent_session_id TEXT, side_parent_leaf_id TEXT);
INSERT INTO chat_sessions VALUES ('foreign','proj_1','claude-code',NULL,NULL,NULL,NULL,NULL,NULL,0,0,NULL,0,NULL,NULL,NULL,1,1,NULL,NULL,NULL,NULL,NULL);
INSERT INTO chat_sessions VALUES ('owner','proj_1','claude-code',NULL,NULL,NULL,NULL,NULL,NULL,0,0,NULL,0,NULL,NULL,NULL,1,1,NULL,NULL,NULL,NULL,NULL);
CREATE TABLE chat_run_wakeups (
                run_id          TEXT NOT NULL,
                chat_session_id TEXT NOT NULL,
                requested_at    INTEGER NOT NULL,
                state           TEXT NOT NULL DEFAULT 'pending',
                claim_token     TEXT,
                claimed_at      INTEGER,
                delivered_at    INTEGER,
                monitoring_alerted INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(run_id, chat_session_id)
            );
INSERT INTO chat_run_wakeups VALUES ('done','owner',400,'delivered',NULL,NULL,500,0);
INSERT INTO chat_run_wakeups VALUES ('failed','foreign',400,'pending',NULL,NULL,NULL,0);
INSERT INTO chat_run_wakeups VALUES ('ssh_job','foreign',400,'pending',NULL,NULL,NULL,1);
INSERT INTO chat_run_wakeups VALUES ('ssh_job','owner',400,'pending',NULL,NULL,NULL,0);
PRAGMA user_version = 1;
