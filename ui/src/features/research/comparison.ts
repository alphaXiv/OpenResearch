import type { Run } from "../../api";

export interface RunMetrics {
  version: 1;
  dataset: string;
  split: string;
  metrics: Record<string, number>;
  parameters: Record<string, string | number | boolean | null>;
  seed: number | null;
}
export interface ComparisonRow {
  id: string;
  runs: Run[];
  measurement: RunMetrics | undefined;
  metrics: Record<string, { mean: number; sd: number | null; n: number }>;
}

function sortedObject(value: Record<string, unknown>) {
  return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)));
}

export function summarize(values: number[]) {
  const finite = values.filter(Number.isFinite);
  if (!finite.length) return null;
  const mean = finite.reduce((total, value) => total + value / finite.length, 0);
  const sd = finite.length > 1
    ? Math.hypot(...finite.map(value => (value - mean) / Math.sqrt(finite.length - 1)))
    : null;
  if (!Number.isFinite(mean)) return null;
  return { mean, sd: sd != null && Number.isFinite(sd) ? sd : null, n: finite.length };
}

export function comparisonRows(runs: Run[], measurements: Record<string, RunMetrics>, groupSeeds: boolean): ComparisonRow[] {
  const groups = new Map<string, Run[]>();
  for (const run of runs) {
    const data = measurements[run.id];
    // Parameters must declare the full experimental configuration. Monitoring
    // metadata and runtime ids are not experimental variables.
    const key = groupSeeds && run.status === "done" && run.commitSha && data?.seed != null
      ? JSON.stringify([run.experimentId, run.commitSha, data.dataset, data.split,
        sortedObject(data.parameters), Object.keys(data.metrics).sort(),
        run.backend?.kind, run.backend?.flavor, run.backend?.image])
      : run.id;
    groups.set(key, [...(groups.get(key) ?? []), run]);
  }
  const rows: ComparisonRow[] = [];
  const append = (group: Run[]) => {
    const measurement = measurements[group[0].id];
    const metrics: ComparisonRow["metrics"] = Object.create(null);
    for (const name of Object.keys(measurement?.metrics ?? {})) {
      const summary = summarize(group.map(run => measurements[run.id]?.metrics[name]));
      if (summary) metrics[name] = summary;
    }
    rows.push({ id: group.map(run => run.id).join(","), runs: group, measurement, metrics });
  };
  for (const group of groups.values()) {
    // Repeated executions of the same seed are not independent seed samples.
    if (new Set(group.map(run => measurements[run.id]?.seed)).size !== group.length) {
      group.forEach(run => append([run]));
    } else append(group);
  }
  return rows;
}

export function metricDelta(row: ComparisonRow, baseline: ComparisonRow, metric: string): number | null {
  if (!row.measurement || !baseline.measurement || row.measurement.dataset !== baseline.measurement.dataset || row.measurement.split !== baseline.measurement.split) return null;
  const current = row.metrics[metric]?.mean;
  const previous = baseline.metrics[metric]?.mean;
  if (current == null || previous == null) return null;
  const delta = current - previous;
  return Number.isFinite(delta) ? delta : null;
}

export function changedParameters(row: ComparisonRow, baseline: ComparisonRow): string[] {
  const current = row.measurement?.parameters ?? {};
  const previous = baseline.measurement?.parameters ?? {};
  return [...new Set([...Object.keys(current), ...Object.keys(previous)])]
    .sort().filter(key => current[key] !== previous[key]);
}
