# OR-304 Compute command coverage

| Compute action | CLI |
|---|---|
| Backend list / configured and effective default | `compute status --json` |
| Set backend and preset/custom flavor | `compute default set BACKEND --flavor VALUE` |
| Clear default | `compute default clear` |
| Machine hardware | `compute show local` |
| SSH aliases and saved targets | `compute show ssh` |
| Edit SSH config | `compute ssh-config show/set` (expected previous content required) |
| Default SSH host | `compute configure ssh --default-host HOST` / `--clear default-host` |
| SSH container or direct host | `compute configure ssh --host HOST --container NAME` / `--clear container` |
| SSH connection / MFA | `compute connect ssh --host HOST` |
| Host and selected-container preflight | `compute test ssh [--host HOST] [--container NAME \| --no-container]` |
| Slurm login, partition, account, time limit | `compute configure slurm --host HOST --partition PARTITION --account ACCOUNT --time-limit 24h` |
| Slurm login/MFA and scheduler checks | `compute connect slurm`, `compute test slurm` |
| Kubernetes contexts, namespace and permissions | `compute show/configure/test k8s` |
| Ray address and health | `compute show/configure/test ray` |
| HF/Modal/Tinker credential status and setup | `compute show/configure/connect/test BACKEND` |
| Remove ORX-saved provider credentials | `compute configure BACKEND --clear credentials` |
| OpenResearch login, organizations and SSH identity | `compute show/connect/test openresearch`; existing `login`, `orgs`, `ssh-key` |
| Cloud catalog | Existing `compute [filters]`, explicit `compute catalog [filters]` |
| Compute activity and history | Existing `runs`, `exp status`, `logs`, `instance list` |
| Reusable custom setup | `compute instructions show/path/set` |

Interactive setup uses existing native flows. HF requires its installed `hf`
CLI; Modal can provision its existing ORX-managed Python environment. Kubernetes
and Ray use their configured credentials; ORX does not invent new login flows.

## Slurm investigation

No hardcoded two-hour limit was found. The requested duration resolves from
`exp run --timeout`, then saved Slurm settings, then the cluster default. `24h`
produces `#SBATCH --time=1-00:00:00` and is now recorded in run metadata.

A separate confirmed problem was found: missing queue/accounting evidence was
converted into a failed run after one minute. The supervisor now retains its
job handle and last known state, records unavailable monitoring, and resumes
polling after recovery. Only an exit status or scheduler terminal state proves
termination. A confirmed cancel request is still attempted during monitoring
loss; no automatic resubmission or cancellation occurs on disconnect.

`exp status` reports the stored requested timeout; `exp status --scheduler` adds
best-effort `sacct` state, effective
time limit, elapsed time, scheduling reason, and exit code. Scheduling reason is
not necessarily the termination cause. Missing accounting remains unknown.
Field definitions: https://slurm.schedmd.com/sacct.html

The original two-hour incident remains unconfirmed without the affected run's
batch script, job/accounting record, supervisor errors, and ORX version. The
controlled transport regression is not a real-cluster or real-MFA test.
