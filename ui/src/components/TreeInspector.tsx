import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { useLocale } from "../locale";
import { useQuery } from "@tanstack/react-query";
import { getRunDiffQuery } from "../queries/files";
import { parseDiff } from "react-diff-view";
import { useState, useMemo } from "react";
import {
  Calendar,
  Check,
  CircleStop,
  Clock3,
  Copy,
  ExternalLink,
  FileCode,
  FolderTree,
  GitBranch,
  Terminal,
  X,
} from "lucide-react";
import {
  fmtDuration,
  githubBranchUrl,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { BackendBadge, GitHubMark } from "./BackendLogos";
import { countChanges } from "./GitDiff";
import { StatusBadge } from "./StatusBadge";
import type { ExperimentView } from "./DetailDrawer";
import type { CodeView } from "./CodeTab";
import type { TabOpenIntent } from "../tabPreview";
import type { ExperimentTreeState } from "../workspaceState";

export function TreeInspector({
  experiment,
  project,
  runs,
  parentSlug,
  onClose,
  onOpenView,
  onOpenCode,
  onCancel,
  onSelectParent,
  tab,
  onTabChange,
}: {
  experiment: Experiment;
  project: Project;
  runs: Run[];
  parentSlug: string | null;
  onClose: () => void;
  onOpenView: (id: string, view: ExperimentView, intent: TabOpenIntent, runId?: string) => void;
  onOpenCode: (
    experimentId: string,
    branch: string,
    view: CodeView,
    intent: TabOpenIntent,
  ) => void;
  onCancel?: (runId: string) => Promise<void>;
  onSelectParent?: (parentId: string) => void;
  tab: ExperimentTreeState["inspectorTab"];
  onTabChange: (tab: ExperimentTreeState["inspectorTab"]) => void;
}) {
  useLocale();
  const [copied, setCopied] = useState(false);
  const [cancelling, setCancelling] = useState(false);

  const sortedRuns = useMemo(
    () => [...runs].sort((a, b) => b.createdAt - a.createdAt),
    [runs],
  );
  const latestRun = sortedRuns[0] ?? null;
  const liveRun = sortedRuns.find((r) => r.status === "running" || r.status === "starting") ?? null;
  const status = liveRun ? runDisplayStatus(liveRun) : latestRun ? runDisplayStatus(latestRun) : "idle";

  const diffRunId = experiment.parentExperimentId && latestRun?.commitSha ? latestRun.id : null;
  const { data: diff } = useQuery({
    ...getRunDiffQuery(diffRunId ?? ""),
    enabled: Boolean(diffRunId),
  });

  const diffFiles = useMemo(() => {
    if (!diff) return [];
    let text = diff.diff;
    if (diff.truncated) {
      const cut = text.lastIndexOf("\ndiff --git ");
      text = cut !== -1 ? text.slice(0, cut + 1) : text.slice(0, text.lastIndexOf("\n") + 1);
    }
    try {
      return text.trim() ? parseDiff(text) : [];
    } catch {
      return [];
    }
  }, [diff]);

  const diffTotals = useMemo(() => {
    let additions = 0;
    let deletions = 0;
    for (const f of diffFiles) {
      const c = countChanges(f);
      additions += c.additions;
      deletions += c.deletions;
    }
    return { additions, deletions, count: diffFiles.length };
  }, [diffFiles]);

  const handleCopyBranch = () => {
    void navigator.clipboard.writeText(experiment.branchName);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const handleStopRun = async (runId: string) => {
    if (!onCancel) return;
    setCancelling(true);
    try {
      await onCancel(runId);
    } finally {
      setCancelling(false);
    }
  };

  return (
    <aside
      className="absolute top-0 end-0 bottom-0 z-40 flex w-96 max-w-full flex-col border-s border-border bg-background shadow-2xl animate-in slide-in-from-right duration-200"
      aria-label="Experiment inspector"
    >
      {/* Header */}
      <div className="flex shrink-0 items-start justify-between gap-3 border-b border-border p-4 bg-surface/50">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-xs font-semibold text-muted uppercase tracking-wider">
              {!experiment.parentExperimentId ? m.tree_baseline() : m.tree_experiment()}
            </span>
            <StatusBadge status={status} />
            {latestRun?.backend && (
              <span className="max-w-28 truncate [&_.backend-detail]:hidden [&_.backend-name]:text-xs [&_svg]:w-3.5 [&_svg]:h-3.5">
                <BackendBadge backend={latestRun.backend} />
              </span>
            )}
          </div>
          <h3 className="m-0 text-base font-semibold text-text truncate" title={experiment.slug}>
            {experiment.slug}
          </h3>
          {experiment.title && (
            <p className="mt-0.5 mb-0 text-xs font-medium text-subtext line-clamp-2" title={experiment.title}>
              {experiment.title}
            </p>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          <button
            type="button"
            onClick={() => onOpenView(experiment.id, "overview", "keepOpen")}
            className="flex h-7 w-7 items-center justify-center rounded-md text-subtext hover:bg-surface hover:text-text cursor-pointer transition-colors"
            title="Open in full tab"
            aria-label="Open in full tab"
          >
            <ExternalLink size={14} />
          </button>
          <button
            type="button"
            onClick={onClose}
            className="flex h-7 w-7 items-center justify-center rounded-md text-subtext hover:bg-surface hover:text-text cursor-pointer transition-colors"
            title={m.file_viewer_dismiss()}
            aria-label={m.file_viewer_dismiss()}
          >
            <X size={15} />
          </button>
        </div>
      </div>

      {/* Quick action bar */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border-variant px-4 py-2 bg-background">
        {latestRun && (
          <button
            type="button"
            onClick={() => onOpenView(experiment.id, "terminal", "preview", latestRun.id)}
            className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1 text-xs font-medium text-text hover:bg-surface transition-colors cursor-pointer"
          >
            <Terminal size={13} />
            <span>{m.tree_view_logs()}</span>
          </button>
        )}
        <button
          type="button"
          onClick={() => onOpenCode(experiment.id, experiment.branchName, "files", "preview")}
          className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1 text-xs font-medium text-text hover:bg-surface transition-colors cursor-pointer"
        >
          <FolderTree size={13} />
          <span>{m.tree_view_code()}</span>
        </button>
        {liveRun && onCancel && (
          <button
            type="button"
            disabled={cancelling}
            onClick={() => void handleStopRun(liveRun.id)}
            className="inline-flex items-center gap-1.5 rounded-md border border-danger/30 bg-danger-subtle px-2.5 py-1 text-xs font-medium text-accent-red hover:bg-danger-subtle/80 cursor-pointer disabled:opacity-50 transition-colors"
          >
            <CircleStop size={13} />
            <span>{cancelling ? m.common_stopping() : m.common_stop()}</span>
          </button>
        )}
        {project.githubEnabled && project.githubOwner && project.githubRepo && (
          <a
            href={githubBranchUrl(project.githubOwner, project.githubRepo, experiment.branchName)}
            target="_blank"
            rel="noopener noreferrer"
            className="ms-auto inline-flex h-7 w-7 items-center justify-center rounded-md border border-border text-subtext hover:bg-surface hover:text-text transition-colors"
            title={m.a11y_open_on_github({ name: ltr(experiment.branchName) })}
          >
            <GitHubMark size={13} />
          </a>
        )}
      </div>

      {/* Tabs */}
      <div className="flex shrink-0 border-b border-border px-4" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "overview"}
          onClick={() => onTabChange("overview")}
          className={`border-b-2 py-2 px-3 text-xs font-medium transition-colors ${
            tab === "overview"
              ? "border-primary text-text"
              : "border-transparent text-subtext hover:text-text"
          }`}
        >
          Overview
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "runs"}
          onClick={() => onTabChange("runs")}
          className={`border-b-2 py-2 px-3 text-xs font-medium transition-colors flex items-center gap-1.5 ${
            tab === "runs"
              ? "border-primary text-text"
              : "border-transparent text-subtext hover:text-text"
          }`}
        >
          <span>{m.tree_view_runs()}</span>
          <span className="rounded-full bg-surface px-1.5 py-0.2 text-muted tabular-nums">
            {runs.length}
          </span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "diff"}
          onClick={() => onTabChange("diff")}
          className={`border-b-2 py-2 px-3 text-xs font-medium transition-colors flex items-center gap-1.5 ${
            tab === "diff"
              ? "border-primary text-text"
              : "border-transparent text-subtext hover:text-text"
          }`}
        >
          <span>{m.git_diff_changed_files()}</span>
          {diffTotals.count > 0 && (
            <span className="text-muted text-xs tabular-nums">
              ({diffTotals.count})
            </span>
          )}
        </button>
      </div>

      {/* Tab Content */}
      <div className="flex-1 min-h-0 overflow-y-auto p-4 space-y-4 text-xs">
        {tab === "overview" && (
          <>
            {experiment.description && (
              <div>
                <h4 className="m-0 mb-1 text-xs font-medium text-muted uppercase tracking-wider">
                  Description
                </h4>
                <p className="m-0 text-text leading-relaxed whitespace-pre-wrap">
                  {experiment.description}
                </p>
              </div>
            )}

            {/* Branch info */}
            <div>
              <h4 className="m-0 mb-1 text-xs font-medium text-muted uppercase tracking-wider">
                Branch
              </h4>
              <div className="flex items-center justify-between gap-2 rounded-md border border-border bg-surface px-2.5 py-1.5">
                <div className="flex items-center gap-1.5 min-w-0 font-mono text-text">
                  <GitBranch size={13} className="shrink-0 text-subtext" />
                  <span className="truncate">{experiment.branchName}</span>
                </div>
                <button
                  type="button"
                  onClick={handleCopyBranch}
                  className="shrink-0 text-subtext hover:text-text cursor-pointer p-0.5"
                  title="Copy branch name"
                >
                  {copied ? <Check size={13} className="text-accent-green" /> : <Copy size={13} />}
                </button>
              </div>
            </div>

            {/* Lineage / Parent info */}
            {parentSlug && (
              <div>
                <h4 className="m-0 mb-1 text-xs font-medium text-muted uppercase tracking-wider">
                  Genealogy
                </h4>
                <div className="flex items-center gap-1.5 text-text">
                  <span className="text-subtext">{m.exp_hover_card_from()}</span>
                  <button
                    type="button"
                    onClick={() => {
                      if (experiment.parentExperimentId && onSelectParent) {
                        onSelectParent(experiment.parentExperimentId);
                      }
                    }}
                    className="font-semibold text-primary hover:underline cursor-pointer"
                  >
                    {parentSlug}
                  </button>
                </div>
              </div>
            )}

            {/* Run command */}
            {experiment.runCommand && (
              <div>
                <h4 className="m-0 mb-1 text-xs font-medium text-muted uppercase tracking-wider">
                  Command
                </h4>
                <pre className="m-0 overflow-x-auto rounded-md border border-border bg-surface p-2 font-mono text-xs text-text">
                  $ {experiment.runCommand}
                </pre>
              </div>
            )}

            {/* Timestamps & summary stats */}
            <div className="space-y-1.5 pt-2 border-t border-border-variant text-subtext">
              <div className="flex items-center gap-2">
                <Calendar size={13} />
                <span>Created {new Date(experiment.createdAt).toLocaleDateString()}</span>
              </div>
              {latestRun && (
                <div className="flex items-center gap-2">
                  <Clock3 size={13} />
                  <span>Last run {timeAgo(latestRun.createdAt)}</span>
                  {latestRun.endedAt != null && (
                    <span className="tabular-nums">
                      · {fmtDuration(latestRun.endedAt - latestRun.createdAt)}
                    </span>
                  )}
                </div>
              )}
            </div>
          </>
        )}

        {tab === "runs" && (
          <div className="space-y-2">
            {sortedRuns.length === 0 ? (
              <p className="m-0 text-subtext text-center py-6">{m.tree_view_no_runs()}</p>
            ) : (
              sortedRuns.map((run, idx) => {
                const runStatus = runDisplayStatus(run);
                const isLive = runStatus === "running" || runStatus === "starting";
                const durationStr = run.endedAt != null ? fmtDuration(run.endedAt - run.createdAt) : null;
                return (
                  <div
                    key={run.id}
                    className="flex flex-col gap-1.5 rounded-lg border border-border-variant bg-surface p-2.5 transition-colors hover:border-border-strong"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-1.5 min-w-0">
                        <span className="font-mono font-medium text-text">
                          #{sortedRuns.length - idx}
                        </span>
                        <StatusBadge status={runStatus} />
                      </div>
                      {run.backend && <BackendBadge backend={run.backend} />}
                    </div>
                    <div className="flex items-center justify-between gap-2 text-subtext">
                      <span className="tabular-nums">{timeAgo(run.createdAt)}</span>
                      {durationStr && <span className="tabular-nums">{durationStr}</span>}
                      <button
                        type="button"
                        onClick={() => onOpenView(experiment.id, "terminal", "preview", run.id)}
                        className="inline-flex items-center gap-1 text-primary hover:underline cursor-pointer"
                      >
                        <Terminal size={11} />
                        <span>Logs</span>
                      </button>
                    </div>
                    {isLive && onCancel && (
                      <button
                        type="button"
                        onClick={() => void handleStopRun(run.id)}
                        className="mt-1 inline-flex items-center justify-center gap-1 rounded border border-danger/30 bg-danger-subtle py-1 text-accent-red hover:bg-danger-subtle/80 cursor-pointer"
                      >
                        <CircleStop size={12} />
                        <span>{m.common_stop()}</span>
                      </button>
                    )}
                  </div>
                );
              })
            )}
          </div>
        )}

        {tab === "diff" && (
          <div className="space-y-3">
            {!experiment.parentExperimentId ? (
              <p className="m-0 text-subtext text-center py-6">
                Baseline experiment (no parent diff available).
              </p>
            ) : diffFiles.length === 0 ? (
              <p className="m-0 text-subtext text-center py-6">
                No committed code changes recorded from parent.
              </p>
            ) : (
              <>
                <div className="flex items-center justify-between rounded-md border border-border bg-surface p-2.5">
                  <span className="font-medium text-text">
                    {diffTotals.count} {diffTotals.count === 1 ? "file" : "files"} changed
                  </span>
                  <div className="flex items-center gap-2 font-mono">
                    <span className="text-accent-green font-semibold">+{diffTotals.additions}</span>
                    <span className="text-accent-red font-semibold">−{diffTotals.deletions}</span>
                  </div>
                </div>

                <div className="space-y-1">
                  {diffFiles.map((f, i) => {
                    const changes = countChanges(f);
                    const path = f.newPath || f.oldPath || `file-${i}`;
                    return (
                      <div
                        key={path}
                        className="flex items-center justify-between gap-2 rounded border border-border-variant bg-surface/50 px-2 py-1.5 font-mono text-xs"
                      >
                        <div className="flex items-center gap-1.5 min-w-0">
                          <FileCode size={12} className="shrink-0 text-subtext" />
                          <span className="truncate text-text" title={path}>
                            {path}
                          </span>
                        </div>
                        <div className="flex items-center gap-1.5 shrink-0 text-xs">
                          {changes.additions > 0 && (
                            <span className="text-accent-green">+{changes.additions}</span>
                          )}
                          {changes.deletions > 0 && (
                            <span className="text-accent-red">−{changes.deletions}</span>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </aside>
  );
}
