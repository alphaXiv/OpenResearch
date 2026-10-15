import { WorkspaceEmptyState } from "./WorkspaceEmptyState";
import { RunComparison } from "../features/research/RunComparison";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { FlaskConical, CircleStop, FolderTree, GitBranch, Search, Terminal, X, Clock3, ArrowDownWideNarrow } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { fmtNumber, runDisplayStatus, timeAgo, type Experiment, type Run } from "../api";
import { StatusBadge } from "./StatusBadge";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { Button, IconButton, Input } from "./ui";
import { ArchiveMenu, type ArchiveActions } from "./ArchiveMenu";
import { experimentRows, filterExperimentRows, type ExperimentFilter, type ExperimentSort } from "../experimentList";
import { useLocale } from "../locale";

export function ExperimentsTable({
  runs,
  experiments,
  archiveActions,
  emptyHint,
  onOpen,
  onOpenLogs,
  onOpenCode,
  onArchive,
  onCancel,
}: {
  runs: Run[];
  experiments: Experiment[];
  archiveActions: Map<string, ArchiveActions>;
  emptyHint?: string;
  onOpen: (experiment: Experiment, intent: TabOpenIntent) => void;
  onOpenLogs: (experimentId: string, runId: string, intent: TabOpenIntent) => void;
  onOpenCode: (experimentId: string, intent: TabOpenIntent) => void;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  onCancel: (runId: string) => Promise<void>;
}) {
  const [pendingCancellation, setPendingCancellation] = useState<ReadonlySet<string>>(new Set());
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<ExperimentFilter>("all");
  const [sort, setSort] = useState<ExperimentSort>("recent");
  const searchRef = useRef<HTMLInputElement>(null);
  const locale = useLocale();
  const rows = useMemo(() => experimentRows(experiments, runs), [experiments, runs]);
  const visibleRows = filterExperimentRows(rows, { search, filter, sort, locale });
  const filters = [
    { id: "all", label: m.chat_panel_all(), count: rows.length },
    { id: "active", label: m.chat_panel_active(), count: rows.filter((row) => row.liveRun).length },
    { id: "failed", label: m.status_failed(), count: rows.filter((row) => row.failed).length },
  ] as const;
  const resetFilters = () => { setSearch(""); setFilter("all"); searchRef.current?.focus(); };

  if (rows.length === 0) {
    return (
      <WorkspaceEmptyState
        icon={FlaskConical}
        title={emptyHint ?? m.experiments_none_yet()}
        description={emptyHint ? undefined : m.experiments_empty_description()}
      />
    );
  }

  async function requestCancel(runId: string) {
    setCancelError(null);
    setPendingCancellation((current) => new Set(current).add(runId));
    try {
      await onCancel(runId);
    } catch (cause) {
      setPendingCancellation((current) => {
        const next = new Set(current);
        next.delete(runId);
        return next;
      });
      setCancelError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <div className="experiments-table-wrap absolute inset-0 overflow-auto bg-background @container">
      <div className="sticky top-0 z-10 flex flex-col gap-3 border-b border-border bg-background p-4">
        <h2 className="m-0 flex items-center gap-2 text-base font-semibold">
          <FlaskConical size={17} className="text-primary" aria-hidden="true" />
          {m.experiments_table_experiments()}
          <span className="rounded-full bg-surface px-2 py-0.5 text-xs font-medium text-subtext tabular-nums">{fmtNumber(rows.length)}</span>
        </h2>
        <div className="relative">
          <Search size={15} className="pointer-events-none absolute start-3 top-3 text-subtext" aria-hidden="true" />
          <Input ref={searchRef} type="search" value={search} onChange={(event) => setSearch(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Escape" && search) { event.preventDefault(); event.stopPropagation(); setSearch(""); } }}
            placeholder={m.experiments_search()} aria-label={m.experiments_search()}
            className="h-10 bg-surface ps-9 pe-10 [&::-webkit-search-cancel-button]:appearance-none" />
          {search && <IconButton className="absolute end-1 top-1" aria-label={m.experiments_clear_search()} onClick={() => { setSearch(""); searchRef.current?.focus(); }}><X size={14} /></IconButton>}
        </div>
        <div className="flex flex-wrap gap-1" role="group" aria-label={m.app_filter_experiments()}>
          {filters.map((item) => <Button key={item.id} size="small" variant="ghost"
            className={filter === item.id ? "bg-primary-subtle text-primary hover:bg-primary-subtle" : "text-subtext"}
            aria-pressed={filter === item.id} onClick={() => setFilter(item.id)}>
            {item.label}<span className="rounded px-1 text-xs tabular-nums">{fmtNumber(item.count)}</span>
          </Button>)}
        </div>
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border-variant pt-3">
          <span className="text-xs text-subtext" role="status">{m.experiments_result_count({ count: fmtNumber(visibleRows.length) })}</span>
          <label className="flex min-w-0 items-center gap-1.5 text-xs text-subtext">
            <ArrowDownWideNarrow size={14} aria-hidden="true" />
            <select aria-label={m.experiments_sort()} value={sort} onChange={(event) => setSort(event.target.value as ExperimentSort)}
              className="max-w-full cursor-pointer rounded-md border-border-variant bg-background py-1 ps-2 pe-6 text-xs text-text">
              <option value="recent">{m.experiments_sort_recent()}</option>
              <option value="name">{m.experiments_sort_name()}</option>
            </select>
          </label>
        </div>
      </div>
      <RunComparison key={experiments[0]?.projectId} runs={runs} experiments={experiments} />
      {cancelError && (
        <div className="experiments-table-error py-2 px-3 text-accent-red text-sm border-b border-b-border" role="alert">
          {m.experiments_table_stop_failed()} {cancelError}
        </div>
      )}
      {visibleRows.length === 0 && <div className="flex flex-col items-center gap-3 px-6 py-12 text-center text-subtext">
          <Search size={28} aria-hidden="true" />
          <p className="m-0 text-sm">{m.experiments_no_matches()}</p>
          <Button onClick={resetFilters}>{m.experiments_reset_filters()}</Button>
        </div>}
      <div className="experiments-table flex w-full flex-col gap-3 bg-canvas p-3 text-sm" role="list" aria-label={m.experiments_table_experiments()}>
        {visibleRows.map(({ experiment, runs: experimentRuns, latestRun, liveRun }) => {
          const logsRun = liveRun ?? latestRun;
          const cancelling = Boolean(
            liveRun && (liveRun.cancelRequested || pendingCancellation.has(liveRun.id)),
          );
          const status = liveRun
            ? cancelling
              ? "cancelling"
              : runDisplayStatus(liveRun)
            : latestRun
              ? runDisplayStatus(latestRun)
              : "idle";

          return (
            <div
              key={experiment.id}
              className="experiment-table-group grid grid-cols-1 gap-3 rounded-xl border border-border-variant bg-background p-4 cursor-pointer transition-colors hover:border-border-strong focus-within:border-primary @2xl:grid-cols-[minmax(0,1fr)_auto]"
              role="listitem"
              onClick={() => onOpen(experiment, "preview")}
              onDoubleClick={() => onOpen(experiment, "keepOpen")}
              onAuxClick={(event) => {
                if (event.button !== 1) return;
                event.preventDefault();
                onOpen(experiment, "keepOpen");
              }}
            >
              <div className="experiment-table-name self-start min-w-0">
                <button
                  type="button"
                  className="experiment-table-title line-clamp-2 w-full text-text font-semibold text-start leading-relaxed wrap-anywhere"
                  title={experiment.title || experiment.slug}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpen(experiment, intent),
                  { stopPropagation: true })}
                >
                  {experiment.title || experiment.slug}
                </button>
                {experiment.archived && <span className="text-muted text-xs">{m.tree_archived()}</span>}
                <span className="experiment-table-subtitle flex items-center min-w-0 gap-1.5 mt-1.5 overflow-hidden text-subtext text-xs [&_>_svg]:shrink-0 [&_code]:min-w-0 [&_code]:overflow-hidden [&_code]:text-ellipsis [&_code]:whitespace-nowrap" title={experiment.branchName}>
                  <GitBranch size={14} aria-hidden="true" />
                  <code>{experiment.branchName}</code>
                </span>
              </div>
              <div className="experiment-table-meta flex flex-wrap items-center gap-x-3 gap-y-2 text-xs @2xl:justify-end">
                <div className="experiment-table-status flex items-center min-w-0">
                  <StatusBadge status={status} />
                </div>
                <div className="experiment-run-summary flex items-center min-w-0 gap-2 text-subtext text-xs">
                  <span>{experimentRuns.length === 1 ? m.experiments_one_run() : m.experiments_run_count({ count: fmtNumber(experimentRuns.length) })}</span>
                </div>
                <div className="experiment-table-latest flex items-center gap-1.5 min-w-0 text-subtext text-xs whitespace-nowrap">
                  <Clock3 size={12} aria-hidden="true" /><span>{latestRun ? timeAgo(latestRun.createdAt) : m.experiments_not_run_yet()}</span>
                </div>
              </div>
              <div
                className="experiment-table-actions flex flex-wrap items-center gap-2 border-t border-border-variant pt-3 @2xl:col-span-2"
                role="group"
                aria-label={m.a11y_actions_for({ name: experiment.title || experiment.slug })}
                onClick={(event) => event.stopPropagation()}
                onDoubleClick={(event) => event.stopPropagation()}
                onAuxClick={(event) => event.stopPropagation()}
              >
                <Button
                  size="small"
                  disabled={!logsRun}
                  title={logsRun ? m.experiments_open_logs() : m.experiments_no_runs_yet()}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => {
                    if (logsRun) onOpenLogs(experiment.id, logsRun.id, intent);
                  }, { stopPropagation: true })}
                >
                  <Terminal size={15} />
                  {m.experiments_table_logs()}
                </Button>
                <Button
                  size="small"
                  title={m.a11y_browse_code_on({ branch: ltr(experiment.branchName) })}
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                    onOpenCode(experiment.id, intent),
                  { stopPropagation: true })}
                >
                  <FolderTree size={15} />
                  {m.experiments_table_code()}
                </Button>
                {liveRun && (
                  <Button
                    size="small"
                    variant="danger"
                    disabled={cancelling}
                    title={cancelling ? m.experiments_stop_requested() : m.experiments_stop_run()}
                    onClick={() => void requestCancel(liveRun.id)}
                  >
                    <CircleStop size={15} />
                    {cancelling ? m.common_stopping() : m.common_stop()}
                  </Button>
                )}
                <ArchiveMenu
                  id={experiment.id}
                  name={experiment.title || experiment.slug}
                  actions={archiveActions.get(experiment.id)!}
                  onArchive={onArchive}
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
