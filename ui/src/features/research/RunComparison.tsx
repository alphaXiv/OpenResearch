import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Experiment, Run } from "../../api";
import { fmtDuration } from "../../api";
import { m } from "../../paraglide/messages.js";
import { workspaceKey } from "../../queries/client";
import { Button } from "../../components/ui";
import { getProjectMetrics, importRunMetrics } from "./api";
import { changedParameters, comparisonRows, metricDelta } from "./comparison";

const number = (value: number) => value.toLocaleString(undefined, { maximumSignificantDigits: 6 });
const example = JSON.stringify({ version: 1, dataset: "dataset-v1", split: "test", metrics: { f1: 0.82, loss: 0.31 }, parameters: { learning_rate: 0.001, batch_size: 32 }, seed: 42 }, null, 2);

export function RunComparison({ runs, experiments }: { runs: Run[]; experiments: Experiment[] }) {
  const projectId = experiments[0]?.projectId ?? "";
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [baselineId, setBaselineId] = useState("");
  const [groupSeeds, setGroupSeeds] = useState(false);
  const [importId, setImportId] = useState("");
  const [replace, setReplace] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const measurements = useQuery({
    queryKey: workspaceKey("researchMetrics", projectId),
    queryFn: ({ signal }) => getProjectMetrics(projectId, signal),
    enabled: open && Boolean(projectId),
    refetchInterval: open ? 15_000 : false,
  });
  const names = new Map(experiments.map(experiment => [experiment.id, experiment.title || experiment.slug]));
  const visibleRuns = runs.filter(run => names.has(run.experimentId)).sort((a, b) => b.createdAt - a.createdAt);
  const rows = comparisonRows(visibleRuns.filter(run => selected.has(run.id)), measurements.data ?? {}, groupSeeds);
  const baseline = rows.find(row => row.id === baselineId) ?? rows[0];
  const metrics = [...new Set(rows.flatMap(row => Object.keys(row.metrics)))].sort();
  const label = (run: Run) => `${names.get(run.experimentId) ?? run.experimentId} · ${run.id.slice(0, 8)}`;

  async function importFile(file: File) {
    if (!importId || saving) return;
    setSaving(true); setError(null);
    try {
      if (file.size > 64 * 1024) throw new Error(m.research_file_too_large());
      await importRunMetrics(importId, JSON.parse(await file.text()), replace);
      await measurements.refetch();
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setSaving(false); }
  }

  return <details className="border-b border-border p-4" open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer font-medium">{m.research_compare_title()}</summary>
    {open && <div className="mt-3 space-y-4 text-sm">
      <p className="text-subtext">{m.research_compare_hint()}</p>
      {(error || measurements.error) && <p role="alert" className="text-accent-red">{error ?? measurements.error?.message}</p>}
      <div className="flex flex-wrap items-center gap-3">
        <Button size="small" disabled={measurements.isFetching} onClick={() => void measurements.refetch()}>{m.settings_page_refresh()}</Button>
        <label className="flex items-center gap-2"><input type="checkbox" checked={groupSeeds} onChange={event => setGroupSeeds(event.target.checked)} />{m.research_group_seeds()}</label>
      </div>
      <p className="text-subtext">{m.research_seed_hint()}</p>
      <div className="max-h-64 overflow-auto rounded border border-border p-2">
        {visibleRuns.length === 0 && <p>{m.research_no_runs()}</p>}
        {visibleRuns.map(run => <label key={run.id} className="flex items-center gap-2 py-1">
          <input type="checkbox" checked={selected.has(run.id)} onChange={event => {
            const checked = event.target.checked;
            setSelected(previous => { const next = new Set(previous); if (checked) next.add(run.id); else next.delete(run.id); return next; });
          }} />
          <span>{label(run)} · {run.status} · {measurements.data?.[run.id] ? m.research_metrics_available() : m.research_metrics_missing()}</span>
        </label>)}
      </div>
      {baseline && <>
        <label className="flex flex-wrap items-center gap-2">{m.research_baseline()}
          <select className="max-w-full rounded border border-border bg-background p-2" value={baseline.id} onChange={event => setBaselineId(event.target.value)}>
            {rows.map(row => <option key={row.id} value={row.id}>{label(row.runs[0])} · n={row.runs.length}</option>)}
          </select>
        </label>
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-start text-sm">
            <thead><tr className="border-b border-border text-subtext">
              {[m.research_run(), m.research_backend(), m.research_elapsed(), m.research_context(), ...metrics, m.research_parameters()].map((heading, index) => <th className="p-2 text-start" key={index}>{heading}</th>)}
            </tr></thead>
            <tbody>{rows.map(row => {
              const run = row.runs[0];
              const changed = changedParameters(row, baseline);
              const params = row.measurement?.parameters ?? {};
              const parameterKeys = [...new Set([...Object.keys(params), ...changed])].sort();
              return <tr key={row.id} className={row.id === baseline.id ? "border-b border-border bg-surface" : "border-b border-border"}>
                <td className="p-2 align-top"><div>{label(run)}</div><div className="text-subtext">{run.status} · {run.commitSha?.slice(0, 8) ?? "—"}</div><div className="text-subtext">n={row.runs.length} · seed: {row.runs.map(item => measurements.data?.[item.id]?.seed ?? "—").join(", ")}</div></td>
                <td className="p-2 align-top">{String(run.backend?.kind ?? "—")}<br />{String(run.backend?.flavor ?? "")}</td>
                <td className="p-2 align-top">{row.runs.map(item => item.endedAt != null ? fmtDuration(Math.max(0, item.endedAt - item.createdAt)) : "—").join(" / ")}</td>
                <td className="p-2 align-top">{row.measurement ? `${row.measurement.dataset} / ${row.measurement.split}` : "—"}</td>
                {metrics.map(metric => {
                  const value = row.metrics[metric];
                  const delta = metricDelta(row, baseline, metric);
                  return <td key={metric} className="whitespace-nowrap p-2 align-top tabular-nums">
                    {value ? <>{number(value.mean)}{value.sd != null && ` ± ${number(value.sd)}`}</> : "—"}
                    {row.id !== baseline.id && <div className="text-subtext">Δ {delta == null ? "—" : `${delta > 0 ? "+" : ""}${number(delta)}`}</div>}
                  </td>;
                })}
                <td className="p-2 align-top">{parameterKeys.length ? parameterKeys.map(key => <div key={key} className={changed.includes(key) ? "font-semibold text-accent-amber" : "text-subtext"}>{key}: {Object.hasOwn(params, key) ? JSON.stringify(params[key]) : "—"}{changed.includes(key) && " *"}</div>) : "—"}</td>
              </tr>;
            })}</tbody>
          </table>
        </div>
        <p className="text-subtext">{m.research_delta_hint()}</p>
      </>}
      <details className="rounded border border-border p-3">
        <summary className="cursor-pointer">{m.research_import_title()}</summary>
        <p className="text-subtext">{m.research_import_hint()}</p>
        <pre className="overflow-auto rounded bg-surface p-3">{example}</pre>
        <code className="block overflow-auto py-2">orx exp metrics &lt;experiment-id&gt; --run &lt;run-id&gt; --file metrics.json</code>
        <div className="mt-2 flex flex-wrap items-center gap-3">
          <select aria-label={m.research_run()} className="max-w-full rounded border border-border bg-background p-2" disabled={saving} value={importId} onChange={event => { setImportId(event.target.value); setReplace(false); }}>
            <option value="">{m.research_select_run()}</option>
            {visibleRuns.map(run => <option value={run.id} key={run.id}>{label(run)}</option>)}
          </select>
          <label className="flex items-center gap-2"><input type="checkbox" checked={replace} disabled={saving} onChange={event => setReplace(event.target.checked)} />{m.research_replace()}</label>
          <input type="file" accept=".json,application/json" aria-label={m.research_import_title()} disabled={!importId || saving} className="max-w-full" onChange={event => {
            const file = event.target.files?.[0]; event.target.value = "";
            if (file) void importFile(file);
          }} />
          {saving && <span role="status">{m.research_importing()}</span>}
        </div>
      </details>
    </div>}
  </details>;
}
