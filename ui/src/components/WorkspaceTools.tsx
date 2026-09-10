import { type Experiment, type Run, runDisplayStatus, fmtNumber } from "../api";
import { activeWorkspaceRuns } from "../workspaceRuns";
import { statusLabel } from "./StatusBadge";
import { getComputeSettingsQuery } from "../queries/settings";
import { TARGET_LABELS } from "../computeTargets";
import { useEffect, useMemo, useRef } from "react";
import { FlaskConical, FolderOpen, Package, GitBranch, Cpu } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getSessionWorktreeQuery } from "../queries/files";
import { countChanges, parseDiffFiles } from "./GitDiff";
import { m } from "../paraglide/messages.js";
import { BackendLogo } from "./BackendLogos";
import { IconButton, MenuItem, StatusIndicator } from "./ui";

export function WorkspaceTools({ expanded, experiments, runs, onOpenExperiment, rightOffset, activeView, projectId, onCompute, sessionId, busy, onChanges, onFiles, onArtifacts, onExperiments }: {
  expanded: boolean;
  experiments: Experiment[];
  runs: Run[];
  onOpenExperiment: (id: string, runId: string) => void;
  rightOffset?: number;
  activeView: "files" | "artifacts" | "experiments" | null;
  projectId: string;
  onCompute: () => void;
  sessionId: string | null;
  busy: boolean;
  onChanges: () => void;
  onFiles: () => void;
  onArtifacts: () => void;
  onExperiments: () => void;
}) {
  const experimentRows = activeWorkspaceRuns(experiments, runs, sessionId);
  const compute = useQuery({ ...getComputeSettingsQuery(projectId), enabled: expanded });
  const defaultBackend = compute.data?.configuredDefaultBackend ?? compute.data?.defaultBackend;
  const computeLabel = defaultBackend ? TARGET_LABELS[defaultBackend]() : compute.isPending ? "…" : compute.isError ? m.model_picker_unavailable() : m.settings_not_set();
  const items = [
    { id: "files", label: m.app_files(), Icon: FolderOpen, onClick: onFiles },
    { id: "artifacts", label: m.app_artifacts(), Icon: Package, onClick: onArtifacts },
    { id: "experiments", label: m.app_experiments(), Icon: FlaskConical, onClick: onExperiments },
  ];
  return (
    <div className="workspace-tools absolute end-3.5 top-7 z-30" style={{ insetInlineEnd: rightOffset }}>
      {!expanded ? (
        <nav aria-label={m.workspace_tools_heading()} className="flex items-center justify-end gap-3">
          {items.map(({ id, label, Icon, onClick }) => (
            <IconButton key={id} active={activeView === id} className="text-text [&.active]:text-text" data-tip={label} data-tip-align={id === "experiments" ? "end" : undefined} aria-label={label} aria-pressed={activeView === id} onClick={onClick}>
              <Icon size={15} />
            </IconButton>
          ))}
        </nav>
      ) : (
        <nav aria-label={m.workspace_tools_heading()} className="workspace-tools-card flex w-60 flex-col gap-0.5 rounded-xl border border-border bg-background px-1.5 py-2 shadow-elevated">
          <h2 className="m-0 px-2 pt-1 pb-2 text-sm font-normal text-subtext">{m.workspace_tools_heading()}</h2>
          {items.filter((item) => item.id !== "files").map(({ id, label: itemLabel, Icon, onClick }) => (
            <MenuItem key={id} className="min-h-7 py-1"
              data-onboarding={id === "artifacts" ? "nav-artifacts" : undefined}
              onClick={onClick}>
              <span className="flex items-center gap-4"><Icon size={15} />{itemLabel}</span>
            </MenuItem>
          ))}
          <MenuItem className="py-1" onClick={onCompute}>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="flex items-center gap-4 text-sm text-text"><Cpu size={15} className="shrink-0" />{m.workspace_default_compute()}</span>
              <span className="flex items-center gap-1.5 ps-[31px] text-menu text-subtext">
                {defaultBackend && <BackendLogo kind={`${defaultBackend}_job`} size={12} />}
                <span className="wrap-anywhere">{computeLabel}</span>
              </span>
            </span>
          </MenuItem>
          <div className="mt-2 border-t border-border/50 pt-3">
            <h2 className="m-0 px-2 pt-1 pb-2 text-sm font-normal text-subtext">{m.workspace_this_worktree()}</h2>
            <MenuItem className="min-h-7 py-1" onClick={onFiles}>
              <span className="flex items-center gap-4"><FolderOpen size={15} />{m.app_files()}</span>
            </MenuItem>
            {sessionId && <ChatBranch key={sessionId} sessionId={sessionId} busy={busy} onChanges={onChanges} />}
            {experimentRows.length > 0 && (
              <div>
                <h2 className="m-0 flex items-center gap-4 px-2 py-1 text-sm font-normal text-text">
                  <span className="flex w-[15px] shrink-0 justify-center"><StatusIndicator tone="success" live /></span>
                  <span>{m.workspace_active_experiments()}</span>
                </h2>
                {experimentRows.map((row) => (
                  <MenuItem key={row.run.id} className="min-h-7 py-1" onClick={() => onOpenExperiment(row.experiment.id, row.run.id)}>
                    <span className="min-w-0 truncate ps-[31px] text-menu text-subtext" title={`${row.experiment.title || row.experiment.slug} · ${statusLabel(runDisplayStatus(row.run))}`}>{row.experiment.title || row.experiment.slug}</span>
                  </MenuItem>
                ))}
              </div>
            )}
          </div>
        </nav>
      )}
    </div>
  );
}

function ChatBranch({ sessionId, busy, onChanges }: { sessionId: string; busy: boolean; onChanges: () => void }) {
  const worktree = useQuery({ ...getSessionWorktreeQuery(sessionId), refetchInterval: busy ? 5_000 : false });
  const wasBusy = useRef(busy);
  const { refetch } = worktree;
  useEffect(() => {
    if (wasBusy.current && !busy) void refetch();
    wasBusy.current = busy;
  }, [busy, refetch]);
  const wt = worktree.data;
  const counts = useMemo(() => {
    if (!wt?.diff || wt.diff.truncated) return null;
    const parsed = parseDiffFiles(wt.diff.diff, false);
    if (parsed.failed) return null;
    return parsed.files.map(countChanges).reduce((total, file) => ({
      additions: total.additions + file.additions,
      deletions: total.deletions + file.deletions,
    }), { additions: 0, deletions: 0 });
  }, [wt?.diff]);
  if (!wt?.exists) return null;
  return (
    <MenuItem className="min-h-7 py-1" onClick={onChanges} title={wt.branch ?? m.settings_detached()}>
      <span className="flex min-w-0 items-center gap-4">
        <GitBranch size={15} className="shrink-0" />
        <span>{m.code_browser_header_changes()}</span>
      </span>
      {counts ? (
        <span className="flex shrink-0 gap-1 text-xs tabular-nums">
          <span className="text-accent-green">+{fmtNumber(counts.additions)}</span>
          <span className="text-accent-red">−{fmtNumber(counts.deletions)}</span>
        </span>
      ) : (
        <span className="shrink-0 text-xs text-subtext">{wt.files?.length === 1 ? m.git_diff_one_changed_file() : m.git_diff_changed_file_count({ count: fmtNumber(wt.files?.length ?? 0) })}</span>
      )}
    </MenuItem>
  );
}
