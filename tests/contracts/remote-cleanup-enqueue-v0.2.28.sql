-- v0.2.27 and v0.2.28 queue_remote_cleanups for one run, with REMOTE_RUNS inlined.
INSERT INTO remote_cleanups (run_id, backend_json, deleted, next_attempt_at)
SELECT id, CASE WHEN json_extract(launch_spec, '$.options.container') IS NULL
    THEN backend_json ELSE json_set(backend_json, '$.sshContainer',
      json(json_extract(launch_spec, '$.options.container'))) END, ?2, ?3
FROM runs WHERE id = ?1 AND CASE WHEN json_valid(backend_json) THEN
    json_extract(backend_json, '$.kind') IN ('ssh_job', 'slurm_job')
    OR json_extract(backend_json, '$.handle.type') IN ('Ssh', 'Slurm') END
ON CONFLICT(run_id) DO UPDATE SET backend_json = excluded.backend_json,
  deleted = MAX(deleted, excluded.deleted), attempts = 0,
  next_attempt_at = excluded.next_attempt_at
