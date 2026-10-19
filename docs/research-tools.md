# Research tools in this fork

## Compute diagnostics

Run `orx compute doctor colab --json`, or open **Settings → Compute → Compute
diagnostics**. The report identifies the running executable, version and build
channel, then checks platform support, the Colab CLI, saved credentials and a
live authenticated request to Google's assigned-runtime endpoint. It refreshes
the cached login before that request. It does not allocate a runtime or launch
an experiment. A failed check includes a recovery action; the CLI exits nonzero
when the backend is not ready. The whole diagnostic is limited to 45 seconds.

Other backends reuse their existing readiness checks. The CLI accepts the same
host/container/address overrides as `orx compute test`; dashboard diagnostics
use saved settings. A passing probe establishes connectivity at that moment,
not future quota, accelerator availability or successful job execution.

After pulling source changes, rebuild with `cargo build --release --locked` and
restart the dashboard with that binary. An older binary can report the same
package version while lacking newer source changes; inspect the executable path
in the diagnostic rather than relying on the version alone.

## Structured measurements and run comparison

Open a project's **Experiments → Compare runs**, select runs and choose a
baseline. The table shows commit, backend, elapsed time, dataset/split, metrics
and changed parameters. Elapsed time is creation-to-completion, including setup.

Import a JSON file through the panel or bind it explicitly to an existing run:

```sh
orx exp metrics EXPERIMENT_ID --run RUN_ID --file metrics.json
orx exp metrics EXPERIMENT_ID --run RUN_ID
# Replacing existing measurements always requires an explicit option:
orx exp metrics EXPERIMENT_ID --run RUN_ID --file metrics.json --replace
```

```json
{
  "version": 1,
  "dataset": "dataset-name-revision-1",
  "split": "test",
  "metrics": { "f1": 0.82, "loss": 0.31 },
  "parameters": { "learning_rate": 0.001, "batch_size": 32 },
  "seed": 42
}
```

`--file -` reads stdin. Imports accept at most 64 KiB and 128 finite numeric
metrics. Parameters must be scalar JSON values; seed belongs in its own field
and must fit a JSON safe integer (±9007199254740991). Omitted seed is allowed.
Record the complete experimental configuration, including data preprocessing,
model/checkpoint and evaluation protocol. Give different units different metric
names, and include the dataset revision in `dataset`. Values come from explicit
imports; this feature does not infer measurements from prose or log text.

Seed grouping is opt-in. Only completed runs with the same experiment, commit,
dataset, split, parameters, metric names and backend kind/flavor/image can join.
Missing commits/seeds and duplicate seeds remain individual rows. The display
uses mean ± **sample** standard deviation; one sample has no standard deviation.
This is descriptive, not a significance test or proof of reproducibility.
Other hardware/environment differences must be recorded in parameters.

Missing measurements appear as `—`, never zero. Delta means current minus
baseline, in the metric's units; it appears only for matching dataset and split.
The UI does not assume whether larger or smaller is better. Parameter changes
are highlighted; differing protocols can still make a delta scientifically
inappropriate even if dataset/split match.

Measurements live in the optional local SQLite table `orx_run_metrics_v1`, with
a run-deletion cleanup trigger. Existing `runs` columns and schema version are
unchanged. Normal database backups include measurements. Core run queries in
older builds ignore this extension; validate migrations when updating upstream.

## Deep research across harnesses

The bundled [orx-deep-research skill](../agent-skills/orx-deep-research/SKILL.md)
is available to new agent sessions and through:

```sh
orx skill deep-research
orx skill deep-research/references/researcher.md
orx skill deep-research/references/report-writer.md
orx agent harnesses --json
```

Use `/deep-research` in a session that supports skill commands, or ask the agent
to use `orx-deep-research`. For example: “Dùng deep-research so sánh các phương
pháp X và Y; ưu tiên bằng chứng thực nghiệm, kết hợp các harness đang có, và lưu
báo cáo tiếng Việt.” Restart an existing session if it has an older skill copy.

The coordinator discovers actual installed harnesses and permitted review
targets, prefers independent vendors where available, assigns separate notes
files, and gives synthesis to one writer. It respects the existing five-helper
limit and forbids nested helpers. It records real harness/model contributions,
uses at most one gap-filling research round, preserves earlier reports and saves
notes/reports under the session artifacts directory. It falls back transparently
when only one harness or no delegation mechanism is available. The skill does
not install providers, change credentials, allocate compute or publish reports.

See [fork maintenance](fork-maintenance.md) for the extension boundaries and
update checks.

## Validation of this change

Local checks on 2026-10-11: Rust formatting, Clippy with warnings denied, release
build, UI typecheck/catalog/style checks, UI build and skill validation passed.
The serial Rust run passed 1,131 unit tests and 21 integration tests, with three
existing fixture-dependent tests ignored and
`foreign_servers_skip_servers_pointed_at_a_database` explicitly excluded after
it failed independently in this environment. The full unfiltered suite is
therefore not claimed green. A parallel run also hit the existing SSH
process-cleanup test; its serial run passed.

All 262 UI tests passed using the installed TypeScript compiler through a
temporary Node loader: this machine's Node 22 lacks native TypeScript stripping.
The dev-slot helper tests passed 8/9; the remaining test requires the missing
`sqlite3` executable. Chart and release-note script tests passed. These
environment workarounds do not change CI or remove any checks.

A live Colab doctor request passed all stages without allocating compute.
The comparison panel opened in an isolated demo dev-slot and the metrics API
accepted synthetic demo data. Browser access became unavailable before the
remaining visual interactions could be verified. The dev-slot was stopped.
The deep-research CLI and both role references were tested; an actual paid
multi-harness research session was not launched as part of installation.
