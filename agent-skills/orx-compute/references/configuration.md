# Configure compute

Settings and custom instructions belong to this machine, across all projects.
Configuration changes affect future launches, not existing runs. Only change the
backend when requested; saving credentials does not select a new default.

```sh
orx compute status --json
orx compute show ssh --json
orx compute configure ssh --default-host lab
orx compute configure ssh --host lab --container research
orx compute configure ssh --host lab --clear container
orx compute test ssh --host lab                     # tests saved container too
orx compute test ssh --host lab --no-container      # explicitly tests host
orx compute connect ssh --host lab                 # interactive login/MFA
orx compute configure slurm --host cluster --partition gpu --account lab --time-limit 24h
orx compute configure slurm --clear time-limit     # cluster chooses
orx compute connect slurm --host cluster
orx compute configure k8s --context research --namespace experiments
orx compute configure ray --address http://127.0.0.1:8265
orx compute default set slurm --flavor h100:1
orx compute default clear
orx compute catalog --gpu H100_SXM --count 1 --json
```

Use `--help` on each command for flags. Supplied fields change; omitted fields
stay unchanged. `--clear <field>` explicitly removes a setting. `status` is a
configuration summary, not proof of connectivity. `test` returns `ready` and
exits nonzero when prerequisites fail. Its checks do not launch training jobs.
Modal checks the local SDK and credential presence; it does not verify token
validity with Modal.

HF, Tinker and Modal credentials can come from `configure <backend>
--credentials-file -` (stdin), or a private JSON file. Shapes: HF `{"token":"…"}`,
Tinker `{"key":"…"}`, Modal `{"tokenId":"…","tokenSecret":"…"}`. Never put
credentials in command arguments, custom instructions, or committed files.
`connect hf` uses `hf auth login`; `connect modal` and `connect tinker` prompt
without echoing credentials. `connect openresearch` reuses login and SSH-key
registration. Interactive login requires a terminal and does not support JSON.
`configure <backend> --clear credentials` removes ORX-saved credentials; process
environment and provider-owned credential stores can still take precedence.

To change SSH aliases, read `ssh-config show --json` and retain its exact content
in a private file. Write the modified config using `ssh-config set --file
<new-file> --previous-file <original-file>`. A stale original is rejected; reread
and reconcile instead of overwriting someone else's edit.

## Maintain the custom recipe

```sh
orx compute instructions show --json
orx compute instructions path
orx compute instructions set --file recipe.md --expected-revision <revision>
```

There is one canonical `<ORX config directory>/compute/CUSTOM.md`; it is not a
copy in each session's generated skills. `path` creates it empty if absent.
`set --file -` reads stdin. Use the revision from the last read; a conflict means
reread and reconcile. Direct edits at the returned path are also supported.
Skill refreshes, upgrades, and session cleanup do not own or delete this file.

Keep a short section for each backend/host: verified environment activation,
data paths, relevant constraints, and documentation links. Example:

```markdown
## SSH: lab
- Use the saved `research` container.
- Source `/opt/conda/etc/profile.d/conda.sh`; activate `research`.
- Dataset: `/datasets/project-a` (read-only).
- Keep setup in committed `scripts/run.sh`.
```

Read current settings and the recipe each time; an earlier session prompt can
be stale. Markdown is guidance, never an automatically executed setup script.
Preserve the fixed run command and launch through `orx exp run`.

## Slurm connection loss

A disconnected monitor does not prove the scheduler stopped the job. Reconnect
with `orx compute connect slurm --host <alias>`, then inspect the existing run
with `orx exp status <expId> --scheduler` (omit `--scheduler` for offline status).
Do not resubmit merely because logs stopped.
Status includes the requested timeout and available scheduler accounting data.
Absent accounting is unknown; a scheduler `TIMEOUT` is evidence of termination.
