-- v0.2.27 and v0.2.28 due_remote_cleanups with eligibility and retention inlined.
SELECT run_id, backend_json, deleted FROM remote_cleanups
WHERE next_attempt_at <= ?1
  AND NOT EXISTS (SELECT 1 FROM runs WHERE id = run_id AND
      (contact IS NOT NULL OR status IN ('submitting','starting','running')))
  AND NOT EXISTS (SELECT 1 FROM log_collections WHERE run_id = remote_cleanups.run_id AND done = 0)
  AND (deleted = 1 OR run_id IN (SELECT id FROM (
    SELECT id, status FROM (
      SELECT id, status, ROW_NUMBER() OVER
        (PARTITION BY project_id ORDER BY created_at DESC, id DESC) AS position
      FROM runs WHERE CASE WHEN json_valid(backend_json) THEN
        json_extract(backend_json, '$.kind') IN ('ssh_job', 'slurm_job')
        OR json_extract(backend_json, '$.handle.type') IN ('Ssh', 'Slurm') END
    ) WHERE position > 3
  )))
ORDER BY next_attempt_at LIMIT ?2
