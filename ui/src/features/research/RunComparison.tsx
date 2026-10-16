import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChartSpline, ChevronRight, GitCommitHorizontal, RefreshCw, Upload } from "lucide-react";
import type { Experiment, Run } from "../../api";
import { fmtDuration, fmtNumber, runDisplayStatus } from "../../api";
import { BackendBadge } from "../../components/BackendLogos";
import { StatusBadge } from "../../components/StatusBadge";
import { Badge, Button, Spinner, Switch } from "../../components/ui";
import { m } from "../../paraglide/messages.js";
import { workspaceKey } from "../../queries/client";
import { getProjectMetrics, importRunMetrics } from "./api";
import { changedParameters, comparisonRows, metricDelta } from "./comparison";

const number = (value: number) => value.toLocaleString(undefined, { maximumSignificantDigits: 6 });
const example = JSON.stringify({ version: 1, dataset: "dataset-v1", split: "test", metrics: { f1: 0.82, loss: 0.31 }, parameters: { learning_rate: 0.001, batch_size: 32 }, seed: 42 }, null, 2);

export function RunComparison({ runs, experiments }: { runs: Run[]; experiments: Experiment[] }) {
  const projectId = experiments[0]?.projectId ?? "";
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const touchedSelectionRef = useRef(false);
  const [baselineId, setBaselineId] = useState("");
  const [groupSeeds, setGroupSeeds] = useState(false);
  const [importId, setImportId] = useState("");
  const [replace, setReplace] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const measurements = useQuery({
    queryKey: workspaceKey("researchMetrics", projectId),
    queryFn: ({ signal }) => getProjectMetrics(projectId, signal),
    enabled: open && Boolean(projectId),
    refetchInterval: open ? 15_000 : false,
  });
  const names = new Map(experiments.map((experiment) => [experiment.id, experiment.title || experiment.slug]));
  const visibleRuns = runs.filter((run) => names.has(run.experimentId)).sort((a, b) => b.createdAt - a.createdAt);

  useEffect(() => {
    if (!open || touchedSelectionRef.current || selected.size > 0 || !measurements.data) return;
    const withMetrics = visibleRuns.filter((run) => Boolean(measurements.data?.[run.id])).slice(0, 6);
    const initial = withMetrics.length > 0 ? withMetrics : visibleRuns.slice(0, 4);
    if (initial.length > 0) setSelected(new Set(initial.map((run) => run.id)));
  }, [open, measurements.data, selected.size, visibleRuns]);

  const rows = comparisonRows(visibleRuns.filter((run) => selected.has(run.id)), measurements.data ?? {}, groupSeeds);
  const baseline = rows.find((row) => row.id === baselineId) ?? rows[0];
  const metrics = [...new Set(rows.flatMap((row) => Object.keys(row.metrics)))].sort();
  const label = (run: Run) => `${names.get(run.experimentId) ?? run.experimentId} · ${run.id.slice(0, 8)}`;
  const measuredCount = measurements.data ? visibleRuns.filter((run) => Boolean(measurements.data?.[run.id])).length : null;
  const allSelected = visibleRuns.length > 0 && visibleRuns.every((run) => selected.has(run.id));

  async function importFile(file: File) {
    if (!importId || saving) return;
    setSaving(true);
    setError(null);
    try {
      if (file.size > 64 * 1024) throw new Error(m.research_file_too_large());
      await importRunMetrics(importId, JSON.parse(await file.text()), replace);
      await measurements.refetch();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <details
      className="group/compare border-b border-border bg-background px-4 py-3"
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="flex cursor-pointer list-none items-center gap-2 text-sm font-semibold text-text select-none [&::-webkit-details-marker]:hidden">
        <ChevronRight
          size={15}
          className="shrink-0 text-muted transition-transform duration-150 ease-standard group-open/compare:rotate-90"
          aria-hidden="true"
        />
        <ChartSpline size={16} className="shrink-0 text-primary" aria-hidden="true" />
        <span>{m.research_compare_title()}</span>
        <span className="rounded-full bg-surface px-2 py-0.5 text-xs font-medium text-subtext tabular-nums">
          {open && selected.size > 0
            ? `${fmtNumber(selected.size)}/${fmtNumber(visibleRuns.length)}`
            : measuredCount != null && measuredCount > 0
              ? fmtNumber(measuredCount)
              : fmtNumber(visibleRuns.length)}
        </span>
      </summary>
      {open && (
        <div className="mt-3.5 space-y-4 text-sm">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="m-0 text-xs leading-relaxed text-subtext">{m.research_compare_hint()}</p>
            <div className="flex flex-wrap items-center gap-2.5">
              {visibleRuns.length > 0 && (
                <Button
                  size="small"
                  variant="ghost"
                  className={allSelected ? "bg-primary-subtle text-primary" : "text-subtext"}
                  aria-pressed={allSelected}
                  onClick={() => {
                    touchedSelectionRef.current = true;
                    setSelected(allSelected ? new Set() : new Set(visibleRuns.map((run) => run.id)));
                  }}
                >
                  {m.chat_panel_all()}
                </Button>
              )}
              <label className="inline-flex cursor-pointer items-center gap-2 text-xs font-medium text-text">
                <Switch
                  type="button"
                  checked={groupSeeds}
                  aria-label={m.research_group_seeds()}
                  onClick={() => setGroupSeeds((current) => !current)}
                />
                <span>{m.research_group_seeds()}</span>
              </label>
              <Button size="small" disabled={measurements.isFetching} onClick={() => void measurements.refetch()}>
                <RefreshCw size={12} className={measurements.isFetching ? "animate-[spin_0.9s_linear_infinite]" : ""} />
                {m.settings_page_refresh()}
              </Button>
            </div>
          </div>
          {groupSeeds && <p className="m-0 text-xs text-subtext">{m.research_seed_hint()}</p>}
          {(error || measurements.error) && (
            <div role="alert" className="rounded-md border border-accent-red bg-accent-red-subtle px-3 py-2 text-xs text-accent-red">
              {error ?? measurements.error?.message}
            </div>
          )}
          <div className="flex max-h-56 flex-col gap-1 overflow-y-auto rounded-lg border border-border-variant bg-surface p-1.5">
            {visibleRuns.length === 0 && <p className="m-0 px-2 py-3 text-center text-xs text-subtext">{m.research_no_runs()}</p>}
            {visibleRuns.map((run) => {
              const checked = selected.has(run.id);
              const hasMetrics = Boolean(measurements.data?.[run.id]);
              return (
                <label
                  key={run.id}
                  className={`flex cursor-pointer flex-wrap items-center gap-2.5 rounded-md border px-2.5 py-1.5 transition-colors ${
                    checked
                      ? "border-border bg-background shadow-hairline"
                      : "border-transparent bg-transparent hover:bg-background"
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    className="m-0 cursor-pointer"
                    onChange={(event) => {
                      touchedSelectionRef.current = true;
                      const isChecked = event.target.checked;
                      setSelected((previous) => {
                        const next = new Set(previous);
                        if (isChecked) next.add(run.id);
                        else next.delete(run.id);
                        return next;
                      });
                    }}
                  />
                  <span className="min-w-0 flex-1 truncate font-medium text-text">{label(run)}</span>
                  <StatusBadge status={runDisplayStatus(run)} />
                  <BackendBadge backend={run.backend} />
                  <Badge size="small" variant={hasMetrics ? "success" : "default"}>
                    {hasMetrics ? m.research_metrics_available() : m.research_metrics_missing()}
                  </Badge>
                </label>
              );
            })}
          </div>
          {baseline && (
            <>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <label className="flex flex-wrap items-center gap-2 text-xs font-medium text-subtext">
                  <span>{m.research_baseline()}</span>
                  <select
                    className="max-w-full cursor-pointer rounded-md border border-border bg-background px-2.5 py-1 text-xs font-medium text-text"
                    value={baseline.id}
                    onChange={(event) => setBaselineId(event.target.value)}
                  >
                    {rows.map((row) => (
                      <option key={row.id} value={row.id}>
                        {label(row.runs[0])} · n={row.runs.length}
                      </option>
                    ))}
                  </select>
                </label>
                <span className="text-xs text-subtext">{m.research_delta_hint()}</span>
              </div>
              <div className="overflow-x-auto rounded-lg border border-border bg-background">
                <table className="w-full border-collapse text-start text-sm">
                  <thead>
                    <tr className="border-b border-border bg-surface text-xs font-semibold text-subtext">
                      {[
                        m.research_run(),
                        m.research_backend(),
                        m.research_elapsed(),
                        m.research_context(),
                        ...metrics,
                        m.research_parameters(),
                      ].map((heading, index) => (
                        <th className="whitespace-nowrap px-3 py-2 text-start font-semibold" key={index}>
                          {heading}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((row) => {
                      const run = row.runs[0];
                      const isBaseline = row.id === baseline.id;
                      const changed = changedParameters(row, baseline);
                      const params = row.measurement?.parameters ?? {};
                      const parameterKeys = [...new Set([...Object.keys(params), ...changed])].sort();
                      return (
                        <tr
                          key={row.id}
                          className={`border-b border-border-variant last:border-b-0 transition-colors hover:bg-surface ${
                            isBaseline ? "bg-surface" : ""
                          }`}
                        >
                          <td className="px-3 py-2.5 align-top">
                            <div className="flex flex-wrap items-center gap-1.5 font-medium text-text">
                              <span>{label(run)}</span>
                              {isBaseline && (
                                <Badge size="small" variant="success">
                                  {m.research_baseline()}
                                </Badge>
                              )}
                            </div>
                            <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-subtext">
                              <StatusBadge status={runDisplayStatus(run)} />
                              {run.commitSha && (
                                <span className="inline-flex items-center gap-1 font-mono">
                                  <GitCommitHorizontal size={12} aria-hidden="true" />
                                  <code>{run.commitSha.slice(0, 8)}</code>
                                </span>
                              )}
                            </div>
                            <div className="mt-1 text-xs text-subtext tabular-nums">
                              n={row.runs.length} · seed: {row.runs.map((item) => measurements.data?.[item.id]?.seed ?? "—").join(", ")}
                            </div>
                          </td>
                          <td className="whitespace-nowrap px-3 py-2.5 align-top">
                            <BackendBadge backend={run.backend} />
                          </td>
                          <td className="whitespace-nowrap px-3 py-2.5 align-top font-mono text-xs text-text tabular-nums">
                            {row.runs
                              .map((item) => (item.endedAt != null ? fmtDuration(Math.max(0, item.endedAt - item.createdAt)) : "—"))
                              .join(" / ")}
                          </td>
                          <td className="whitespace-nowrap px-3 py-2.5 align-top">
                            {row.measurement ? (
                              <code className="rounded border border-border-variant bg-surface px-1.5 py-0.5 font-mono text-xs text-text">
                                {row.measurement.dataset} / {row.measurement.split}
                              </code>
                            ) : (
                              <span className="text-subtext">—</span>
                            )}
                          </td>
                          {metrics.map((metric) => {
                            const value = row.metrics[metric];
                            const delta = metricDelta(row, baseline, metric);
                            const deltaTone =
                              delta == null || delta === 0
                                ? "text-subtext"
                                : delta > 0
                                  ? "text-accent-green font-medium"
                                  : "text-accent-red font-medium";
                            return (
                              <td key={metric} className="whitespace-nowrap px-3 py-2.5 align-top font-mono tabular-nums">
                                <div className="font-medium text-text">
                                  {value ? (
                                    <>
                                      {number(value.mean)}
                                      {value.sd != null && <span className="text-xs font-normal text-subtext"> ± {number(value.sd)}</span>}
                                    </>
                                  ) : (
                                    <span className="text-subtext">—</span>
                                  )}
                                </div>
                                {!isBaseline && (
                                  <div className={`mt-0.5 text-xs ${deltaTone}`}>
                                    Δ {delta == null ? "—" : `${delta > 0 ? "+" : ""}${number(delta)}`}
                                  </div>
                                )}
                              </td>
                            );
                          })}
                          <td className="px-3 py-2.5 align-top">
                            {parameterKeys.length ? (
                              <div className="flex flex-wrap gap-1">
                                {parameterKeys.map((key) => {
                                  const isChanged = changed.includes(key);
                                  return (
                                    <span
                                      key={key}
                                      className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-xs ${
                                        isChanged
                                          ? "border-accent-amber bg-accent-amber-subtle font-medium text-accent-amber"
                                          : "border-border-variant bg-surface text-subtext"
                                      }`}
                                    >
                                      {key}: {Object.hasOwn(params, key) ? JSON.stringify(params[key]) : "—"}
                                      {isChanged && " *"}
                                    </span>
                                  );
                                })}
                              </div>
                            ) : (
                              <span className="text-subtext">—</span>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </>
          )}
          <details className="group/import rounded-lg border border-border-variant bg-surface p-3">
            <summary className="flex cursor-pointer list-none items-center gap-2 text-xs font-semibold text-text select-none [&::-webkit-details-marker]:hidden">
              <ChevronRight
                size={14}
                className="shrink-0 text-muted transition-transform duration-150 ease-standard group-open/import:rotate-90"
                aria-hidden="true"
              />
              <Upload size={14} className="shrink-0 text-subtext" aria-hidden="true" />
              <span>{m.research_import_title()}</span>
            </summary>
            <div className="mt-2.5 space-y-2.5 text-xs">
              <p className="m-0 text-subtext">{m.research_import_hint()}</p>
              <pre className="m-0 overflow-auto rounded-md border border-border-variant bg-background p-2.5 font-mono text-xs text-text">{example}</pre>
              <code className="block overflow-auto rounded-md border border-border-variant bg-background px-2.5 py-1.5 font-mono text-xs text-subtext">
                orx exp metrics &lt;experiment-id&gt; --run &lt;run-id&gt; --file metrics.json
              </code>
              <div className="flex flex-wrap items-center gap-3 pt-1">
                <select
                  aria-label={m.research_run()}
                  className="max-w-full cursor-pointer rounded-md border border-border bg-background px-2.5 py-1.5 text-xs text-text"
                  disabled={saving}
                  value={importId}
                  onChange={(event) => {
                    setImportId(event.target.value);
                    setReplace(false);
                  }}
                >
                  <option value="">{m.research_select_run()}</option>
                  {visibleRuns.map((run) => (
                    <option value={run.id} key={run.id}>
                      {label(run)}
                    </option>
                  ))}
                </select>
                <label className="inline-flex cursor-pointer items-center gap-2 text-xs text-text">
                  <Switch
                    type="button"
                    checked={replace}
                    disabled={saving}
                    aria-label={m.research_replace()}
                    onClick={() => setReplace((current) => !current)}
                  />
                  <span>{m.research_replace()}</span>
                </label>
                <input
                  ref={fileInputRef}
                  type="file"
                  accept=".json,application/json"
                  aria-label={m.research_import_title()}
                  disabled={!importId || saving}
                  hidden
                  onChange={(event) => {
                    const file = event.target.files?.[0];
                    event.target.value = "";
                    if (file) void importFile(file);
                  }}
                />
                <Button
                  size="small"
                  variant="primary"
                  disabled={!importId || saving}
                  onClick={() => fileInputRef.current?.click()}
                >
                  {saving ? <Spinner /> : <Upload size={13} />}
                  {saving ? m.research_importing() : m.research_import_title()}
                </Button>
              </div>
            </div>
          </details>
        </div>
      )}
    </details>
  );
}

