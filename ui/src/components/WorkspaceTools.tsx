import { EscapeScope } from "./EscapeScope";
import { type ChatSession, type Experiment, type Run, runDisplayStatus, fmtNumber, timeAgo } from "../api";
import { activeWorkspaceRuns } from "../workspaceRuns";
import { statusLabel } from "./StatusBadge";
import { getComputeSettingsQuery } from "../queries/settings";
import { TARGET_LABELS } from "../computeTargets";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { FlaskConical, FolderOpen, Package, GitBranch, Cpu, MessagesSquare, Plus, Terminal } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getSessionWorktreeQuery } from "../queries/files";
import { countChanges, parseDiffFiles } from "./GitDiff";
import { m } from "../paraglide/messages.js";
import { BackendLogo } from "./BackendLogos";
import { IconButton, MenuItem, StatusIndicator, Spinner, Tooltip } from "./ui";
import { usePopover } from "./ModelPicker";
import { formatForDisplay } from "@tanstack/react-hotkeys";
import { appHotkeys } from "../hotkeys";
import { navigateKeyboardList, navigateKeyboardRow } from "../keyboardNavigation";

export function WorkspaceTools({ expanded, experiments, runs, onOpenExperiment, rightOffset, activeView, projectId, onCompute, sessionId, busy, onChanges, onFiles, onTerminal, onArtifacts, onExperiments, onSideChat, sessions, onSelectSideChat }: {
  expanded: boolean;
  experiments: Experiment[];
  runs: Run[];
  onOpenExperiment: (id: string, runId: string) => void;
  rightOffset?: number;
  activeView: "files" | "artifacts" | "experiments" | "terminal" | null;
  projectId: string;
  onCompute: () => void;
  sessionId: string | null;
  busy: boolean;
  onChanges: () => void;
  onFiles: () => void;
  onTerminal: () => void;
  onArtifacts: () => void;
  onExperiments: () => void;
  /** Absent until there is a chat to branch from. */
  onSideChat?: () => void;
  sessions: ChatSession[];
  onSelectSideChat: (sessionId: string) => void;
}) {
  const experimentRows = activeWorkspaceRuns(experiments, runs, sessionId);
  const compute = useQuery({ ...getComputeSettingsQuery(projectId), enabled: expanded });
  const defaultBackend = compute.data?.configuredDefaultBackend ?? compute.data?.defaultBackend;
  const computeLabel = defaultBackend ? TARGET_LABELS[defaultBackend]() : compute.isPending ? "…" : compute.isError ? m.model_picker_unavailable() : m.settings_not_set();
  const items = [
    { id: "files", label: m.app_files(), Icon: FolderOpen, onClick: onFiles, shortcut: appHotkeys["files.open"].hotkey },
    { id: "artifacts", label: m.app_artifacts(), Icon: Package, onClick: onArtifacts, shortcut: appHotkeys["artifacts.open"].hotkey },
    { id: "experiments", label: m.app_experiments(), Icon: FlaskConical, onClick: onExperiments, shortcut: appHotkeys["experiments.open"].hotkey },
    { id: "terminal", label: m.workspace_terminal(), Icon: Terminal, onClick: onTerminal, shortcut: appHotkeys["terminal.open"].hotkey },
  ].map((item) => ({
    ...item,
    ariaShortcut: typeof item.shortcut === "string" ? `${item.shortcut.replace("Mod+", "Meta+")} ${item.shortcut.replace("Mod+", "Control+")}` : undefined,
  }));
  return (
    <>
    <nav aria-label={m.workspace_tools_heading()} className="workspace-mobile-nav hidden">
      {items.map(({ id, label, Icon, onClick }) => (
        <button key={id} type="button" aria-pressed={activeView === id} onClick={onClick}>
          <Icon size={20} aria-hidden="true" />
          <span>{label}</span>
        </button>
      ))}
      {onSideChat && sessionId && <SideChatPicker parentId={sessionId} sessions={sessions} onSelect={onSelectSideChat} onCreate={onSideChat} mobile />}
    </nav>
    <div className="workspace-tools absolute end-3.5 top-2 mac-titlebar:top-1.5 z-30" style={{ insetInlineEnd: rightOffset }}>
      {!expanded ? (
        <nav aria-label={m.workspace_tools_heading()} onKeyDown={navigateKeyboardRow} className="flex items-center justify-end gap-1">
          {items.map(({ id, label, Icon, onClick, shortcut, ariaShortcut }) => (
            <IconButton key={id} data-keyboard-horizontal-item active={activeView === id} className="text-text" data-tip={shortcut ? `${label} (${formatForDisplay(shortcut)})` : label} data-tip-align={id === "terminal" ? "end" : undefined} aria-label={label} aria-keyshortcuts={ariaShortcut} aria-pressed={activeView === id} onClick={onClick}>
              <Icon size={16} />
            </IconButton>
          ))}
          {onSideChat && sessionId && <SideChatPicker parentId={sessionId} sessions={sessions} onSelect={onSelectSideChat} onCreate={onSideChat} />}
        </nav>
      ) : (
        <nav data-keyboard-list aria-label={m.workspace_tools_heading()} onKeyDown={navigateKeyboardList} className="workspace-tools-card flex w-60 flex-col gap-0.5 rounded-xl border border-border bg-background px-1.5 py-2 shadow-control-subtle">
          <h2 className="m-0 px-2 pt-1 pb-2 text-sm font-normal text-subtext">{m.workspace_tools_heading()}</h2>
          {items.filter((item) => item.id !== "files" && item.id !== "terminal").map(({ id, label: itemLabel, Icon, onClick, shortcut, ariaShortcut }) => (
            <MenuItem key={id} data-keyboard-item className="min-h-7 py-1"
              data-onboarding={id === "artifacts" ? "nav-artifacts" : undefined}
              data-tip={shortcut ? formatForDisplay(shortcut) : undefined}
              aria-label={itemLabel}
              aria-keyshortcuts={ariaShortcut}
              onClick={onClick}>
              <span className="flex items-center gap-4"><Icon size={16} />{itemLabel}</span>
            </MenuItem>
          ))}
          <MenuItem data-keyboard-item className="py-1" data-tip={formatForDisplay(appHotkeys["compute.open"].hotkey ?? [])} aria-label={`${m.workspace_default_compute()}: ${computeLabel}`} aria-keyshortcuts="Meta+Alt+C Control+Alt+C" onClick={onCompute}>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="flex items-center gap-4 text-sm text-text"><Cpu size={16} className="shrink-0" />{m.workspace_default_compute()}</span>
              <span className="flex items-center gap-1.5 ps-8 text-menu text-subtext">
                {defaultBackend && <BackendLogo kind={`${defaultBackend}_job`} size={12} />}
                <span className="wrap-anywhere">{computeLabel}</span>
              </span>
            </span>
          </MenuItem>
          <div className="mt-2 border-t border-border/50 pt-3">
            <h2 className="m-0 px-2 pt-1 pb-2 text-sm font-normal text-subtext">{m.workspace_this_worktree()}</h2>
            {items.filter((item) => item.id === "files" || item.id === "terminal").map(({ id, label, Icon, onClick, shortcut, ariaShortcut }) => (
              <MenuItem key={id} data-keyboard-item className="min-h-7 py-1" data-tip={shortcut ? formatForDisplay(shortcut) : undefined} aria-label={label} aria-keyshortcuts={ariaShortcut} onClick={onClick}>
                <span className="flex items-center gap-4"><Icon size={16} />{label}</span>
              </MenuItem>
            ))}
            {onSideChat && sessionId && (
              <SideChatPicker parentId={sessionId} sessions={sessions} onSelect={onSelectSideChat} onCreate={onSideChat} expanded />
            )}
            {sessionId && <ChatBranch key={sessionId} sessionId={sessionId} busy={busy} onChanges={onChanges} />}
            {experimentRows.length > 0 && (
              <div>
                <h2 className="m-0 flex items-center gap-4 px-2 py-1 text-sm font-normal text-text">
                  <span className="flex w-[15px] shrink-0 justify-center"><StatusIndicator tone="success" live /></span>
                  <span>{m.workspace_active_experiments()}</span>
                </h2>
                {experimentRows.map((row) => (
                  <MenuItem key={row.run.id} data-keyboard-item className="min-h-7 py-1" onClick={() => onOpenExperiment(row.experiment.id, row.run.id)}>
                    <span className="min-w-0 truncate ps-8 text-menu text-subtext" title={`${row.experiment.title || row.experiment.slug} · ${statusLabel(runDisplayStatus(row.run))}`}>{row.experiment.title || row.experiment.slug}</span>
                  </MenuItem>
                ))}
              </div>
            )}
          </div>
        </nav>
      )}
    </div>
    </>
  );
}

function SideChatPicker({ parentId, sessions, onSelect, onCreate, expanded = false, mobile = false }: {
  expanded?: boolean;
  mobile?: boolean;
  parentId: string;
  sessions: ChatSession[];
  onSelect: (sessionId: string) => void;
  onCreate: () => void;
}) {
  const children = sessions.filter((session) => session.sideParentSessionId === parentId)
    .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const openedByHover = useRef(false);
  const { open, setOpen, ref } = usePopover(triggerRef);
  const [filter, setFilter] = useState("");
  const visible = children.filter((session) => (session.title?.trim() || m.side_chat_untitled()).toLowerCase().includes(filter.trim().toLowerCase()));

  useEffect(() => () => clearTimeout(closeTimer.current), []);
  useLayoutEffect(() => {
    if (!open) return;
    const position = () => {
      if (!panelRef.current) return;
      const available = mobile
        ? (triggerRef.current?.getBoundingClientRect().top ?? 0) - 12
        : window.innerHeight - panelRef.current.getBoundingClientRect().top - 8;
      panelRef.current.style.maxHeight = `${Math.max(0, Math.min(384, available))}px`;
    };
    position();
    window.addEventListener("resize", position);
    window.addEventListener("scroll", position, true);
    return () => {
      window.removeEventListener("resize", position);
      window.removeEventListener("scroll", position, true);
    };
  }, [open, mobile]);

  const openPicker = () => {
    clearTimeout(closeTimer.current);
    openedByHover.current = false;
    setFilter("");
    setOpen(true);
    requestAnimationFrame(() => (panelRef.current?.querySelector<HTMLElement>("input") ?? panelRef.current?.querySelector<HTMLElement>("button"))?.focus());
  };
  const closePicker = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };
  const triggerProps = {
    ref: triggerRef,
    type: "button" as const,
    "aria-label": m.side_chat_open(),
    "aria-haspopup": "dialog" as const,
    "aria-expanded": open,
    "data-keyboard-item": true,
    "data-keyboard-horizontal-item": true,
    onClick: () => open && !openedByHover.current ? closePicker() : openPicker(),
    onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        openPicker();
      }
    },
  };

  return (
    <div ref={ref} className={`relative inline-flex${expanded ? " w-full" : ""}`}
      onPointerEnter={(event) => {
        if (event.pointerType !== "mouse") return;
        clearTimeout(closeTimer.current);
        if (!open) {
          openedByHover.current = true;
          setFilter("");
          setOpen(true);
        }
      }}
      onPointerLeave={() => {
        closeTimer.current = setTimeout(() => {
          if (!panelRef.current?.contains(document.activeElement)) setOpen(false);
        }, 150);
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}>
      {mobile ? (
        <button {...triggerProps} className="w-full">
          <MessagesSquare size={20} aria-hidden="true" />
          <span>{m.side_chat_open()}</span>
        </button>
      ) : expanded ? (
        <MenuItem {...triggerProps} active={open} className="min-h-7 py-1 text-text">
          <span className="flex items-center gap-4"><MessagesSquare size={16} />{m.side_chat_open()}</span>
        </MenuItem>
      ) : (
        <IconButton {...triggerProps} size="small" active={open} className="text-text">
          <MessagesSquare size={16} />
        </IconButton>
      )}
      {open && (
        <div ref={panelRef} role="dialog" aria-label={m.side_chat_list({ count: children.length })}
          className={`absolute end-0 ${mobile ? "bottom-[calc(100%_+_4px)]" : "top-[calc(100%_+_4px)]"} z-50 flex w-80 max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-lg border border-border bg-background p-1.5 shadow-menu`}>
          <div className="flex shrink-0 items-center justify-between gap-2 px-2">
            <span className="text-sm font-medium text-text">{m.side_chat_list({ count: children.length })}</span>
            {children.length > 0 && (
              <Tooltip interactive content={m.side_chat_new()} className="shrink-0 cursor-default">
                <IconButton type="button" size="small" aria-label={m.side_chat_new()} onClick={() => { closePicker(); onCreate(); }}>
                  <Plus size={16} />
                </IconButton>
              </Tooltip>
            )}
          </div>
          {children.length > 0 ? (
            <>
              <input aria-label={m.side_chat_search()} placeholder={m.side_chat_search()} value={filter} onChange={(event) => setFilter(event.target.value)} onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    panelRef.current?.querySelector<HTMLElement>("[data-side-chat-result]")?.focus();
                  }
                }}
                className="mb-1 w-full shrink-0 border-b border-border bg-background px-2 py-1.5 text-sm text-text outline-none focus-visible:outline-2 focus-visible:outline-focus" />
              <div className="min-h-0 overflow-y-auto" onKeyDown={(event) => {
                if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
                const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("[data-side-chat-result]"));
                const index = buttons.findIndex((button) => button === document.activeElement);
                if (index < 0) return;
                event.preventDefault();
                if (event.key === "ArrowUp" && index === 0) panelRef.current?.querySelector("input")?.focus();
                else buttons[Math.max(0, Math.min(buttons.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)))]?.focus();
              }}>
                {visible.length === 0 && <div className="px-2 py-2 text-sm text-subtext" role="status">{m.onboarding_not_found()}</div>}
                {visible.map((session) => {
                  const title = session.title?.trim() || m.side_chat_untitled();
                  return (
                    <Tooltip key={session.id} interactive content={title} className="w-full cursor-default">
                      <MenuItem type="button" data-side-chat-result onClick={() => { closePicker(); onSelect(session.id); }}>
                        <span className="flex min-w-0 items-center gap-2">
                          {session.busy ? <Spinner /> : <MessagesSquare size={14} className="shrink-0" />}
                          <span className="truncate">{title}</span>
                        </span>
                        <span className="shrink-0 text-xs text-muted">{timeAgo(session.updatedAt)}</span>
                      </MenuItem>
                    </Tooltip>
                  );
                })}
              </div>
            </>
          ) : (
            <MenuItem type="button" onClick={() => { closePicker(); onCreate(); }}>
              <span className="flex items-center gap-2"><Plus size={14} />{m.side_chat_new()}</span>
            </MenuItem>
          )}
        </div>
      )}
      <EscapeScope enabled={open} onEscape={() => { setOpen(false); triggerRef.current?.focus(); }} />
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
  const branch = wt.branch ?? m.settings_detached();
  return (
    <Tooltip interactive content={branch} className="flex w-full cursor-default">
      <MenuItem className="min-h-7 py-1" onClick={onChanges} aria-description={branch}>
        <span className="flex min-w-0 items-center gap-4">
          <GitBranch size={16} className="shrink-0" />
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
    </Tooltip>
  );
}
