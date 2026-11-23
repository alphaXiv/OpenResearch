# Progress lines: fields, several processes, shell scripts, resumed runs

## Fields of an `::orx::` line

`phase` starts a stage with no step, total or unit until a report gives them;
within a stage, omitted fields keep their last value. `step` is completed work
out of `of`; `t` is seconds on any monotonic clock (only differences count);
measures go in `m`; `row` adds a row to that table; `spend` is cumulative
paid-service USD; `plan` is the stage list, as in `plan.json`.

## The Python helper

Only rank zero reports. The helper takes the rank from `rank=`, else `RANK` or
`SLURM_PROCID`; under a launcher that sets neither, pass `rank=`, or every
process reports and each row counts once per process.

`orx_run.py` flushes a non-finite measure at once, says on stderr when orx will
ignore something it printed (a boolean, a list, a name like `val/loss`), and
never marks a run finished. Resume from a checkpoint with
`progress(total=5000, start=3000)`.

## Stages run by a shell script

A shell script names the stages and relays the progress of tools it can't
change; the Python it starts then calls `progress(total=…)` without `phase`:

```sh
report() { printf '::orx:: %s\n' "$1"; }
report '{"phase":"download","step":0,"of":9,"unit":"shards"}'
python download.py | while IFS= read -r line; do
    printf '%s\n' "$line"
    case $line in "Saved shard"*) n=$((n + 1)); report "{\"step\":$n}" ;; esac
done
report '{"phase":"train"}'
PYTHONUNBUFFERED=1 python train.py
```

Without `PYTHONUNBUFFERED=1`, piped output arrives late and in bursts.
