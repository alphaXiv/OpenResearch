import { useBlocker, useRouter, useRouterState } from "@tanstack/react-router";
import { getTaskWorkspace, parseDestination, safeLocation, taskLocation, type Pane, type TaskWorkspace } from "./workspaceState";
import { getRememberedGlobalWorkspace, globalWorkspaceWriter } from "./workspacePersistence";
import { type ExpViewDef, sameExpTab, type FileViewDef, sameFileTab, fileTabKey, fileScrollKey, persistentFileTab, persistentRightTab, type PlanViewDef, type SubagentViewDef, type CodeTabDef, sameCodeTab, type RightTab, type ContentTab, rightTabKey, withoutTab, isPresent, type RightPaneSessionState, initialRightPaneSessionState, tabPane, paneTab, defaultTaskWorkspace } from "./workspaceTabs";
import { getCachedProjectWorkspace, inheritNewTaskWorkspace, useProjectWorkspace } from "./useProjectWorkspace";
import { m } from "./paraglide/messages.js";
import { getLocale } from "./paraglide/runtime.js";
import { useLocale } from "./locale";
import { autoDir } from "./i18n";
import {
  ChartSpline,
  Check,
  FileCode,
  Filter,
  FlaskConical,
  FolderGit2,
  FolderOpen,
  Maximize2,
  Minimize2,
  Package,
  ScrollText,
  Terminal,
  Users,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  cancelRun,
  DEMO_MAIN_SESSION_ID,
  DEMO_OVERVIEW_ARTIFACT,
  DEMO_RUN_EXPERIMENT_PROMPT,
  getArtifacts,
  getChatMessages,
  getUiState,
  listChatSessions,
  isDemoProjectId,
  listExperiments,
  listProjects,
  listRuns,
  openProject,
  updateUiState,
  type AgentSelection,
  type Experiment,
  type ProjectArtifacts,
  type Project,
  type RuntimeInfo,
  type Run,
  type ChatMessage,
  type UiState,
} from "./api";
import { ChatPanel, findPartById, spawnRowTitle } from "./components/ChatPanel";
import { usePopover } from "./components/ModelPicker";
import { SubagentTab } from "./components/SubagentTab";
import { CodeTab, type CodeView } from "./components/CodeTab";
import { WorktreeTab, type WorktreeView } from "./components/WorktreeTab";
import { ArtifactsTab, findArtifactEntry } from "./components/ArtifactsTab";
import { SkillsTab } from "./components/SkillsTab";
import { ClosableTab } from "./components/ClosableTab";
import { DetailDrawer, type ExperimentView } from "./components/DetailDrawer";
import { FileViewer, type FileScrollPosition } from "./components/FileViewer";
import { confirmFileDiscard, FileBufferSession } from "./fileSync";
import { RailHeader } from "./components/Header";
import { UpdateBanner, useUpdateStatus } from "./components/UpdateBanner";
import { OfflineBanner } from "./components/OfflineBanner";
import { NewProjectDialog } from "./components/ProjectsHome";
import { ExperimentsTable } from "./components/ExperimentsTable";
import { Md } from "./components/Md";
import { SettingsView, type SettingsTab } from "./components/SettingsPage";
import { DemoWelcomeModal } from "./components/Tour";
import { TreeView } from "./components/TreeView";
import { onChatEvent, useOrxEvents } from "./events";
import { closeTab, openTab, type TabOpenIntent } from "./tabPreview";
import { Button, IconButton, MenuItem, showAlert, Spinner } from "./components/ui";
import { CodeTabBody, TabBody } from "./components/layout/TabBody";
import { RemoteStatus } from "./components/RemoteStatus";

const EMPTY_STATE_CLASS_NAME = [
  "empty-state absolute inset-0 flex flex-col items-center",
  "justify-center gap-2.5 p-6 text-center text-subtext",
  "[&_p]:max-w-[46ch] [&_p]:m-0 [&_p]:text-sm [&_p]:leading-normal",
  "[&_p]:text-balance [&_p.empty-state-title]:text-2xl",
  "[&_p.empty-state-title]:font-normal [&_p.empty-state-title]:text-text",
  "[&_p.empty-state-hint]:text-lg [&_p.empty-state-hint]:text-subtext",
].join(" ");

/** Escape a string for literal use inside a RegExp. */
function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Map a path an agent reported to a right-pane file tab. An artifact path under
// the compatibility <data dir>/files/<slug>/ layout is stripped to a relative
// path and tagged source:"artifacts". Otherwise it's a repo/worktree path stripped to
// repo-relative, keeping the session id when it points into a per-session
// worktree. Relative paths name files in the click context's checkout and
// inherit `contextSessionId`; the regex fallbacks encode the
// ~/.cache/openresearch/ layouts from src/local/git.rs:
// worktrees/<project-id>/<session>/… and the legacy repos/<owner>/<repo>/….
function parseFilePath(
  rawPath: string,
  repoPath?: string,
  contextSessionId?: string,
  artifactsDir?: string,
  slug?: string,
): FileViewDef | null {
  let path = rawPath;
  let sessionId: string | undefined;
  const clone = repoPath?.replace(/\/+$/, "");
  const artifacts = artifactsDir?.replace(/\/+$/, "");
  if (path.startsWith("artifacts/")) {
    path = path.slice("artifacts/".length);
    return path ? { path, source: "artifacts" } : null;
  }
  // A home-anchored path (`~` or `~/…`) is disk, never a repo file — the backend
  // expands the `~`, so hand it over verbatim.
  if (path === "~" || path.startsWith("~/")) return { path, source: "abs" };
  // `path` relative to `base` (`""` when equal), else null. macOS symlinks
  // `/tmp`→`/private/tmp` and `/var`→`/private/var`, so an agent-inlined path
  // and the stored dir can differ only by that prefix — strip it on both sides.
  const relUnder = (base: string): string | null => {
    const strip = (p: string) => p.replace(/^\/private(?=\/(?:tmp|var)(?:\/|$))/, "");
    const [p, b] = [strip(path), strip(base)];
    if (p === b) return "";
    return p.startsWith(`${b}/`) ? p.slice(b.length).replace(/^\/+/, "") : null;
  };
  // A relative path names a file in the click context's checkout; the absolute
  // branches below are keyed off the (non-canonical) stored dirs.
  const artifactRel = path.startsWith("/") && artifacts ? relUnder(artifacts) : null;
  const cloneRel = path.startsWith("/") && clone ? relUnder(clone) : null;
  if (!path.startsWith("/")) {
    sessionId = contextSessionId;
  } else if (artifactRel !== null) {
    // Artifact — prefix match against the non-canonical dir the backend
    // surfaced, which mirrors what the agent inlines.
    return artifactRel ? { path: artifactRel, source: "artifacts" } : null;
  } else if (cloneRel !== null) {
    path = cloneRel;
  } else {
    // Artifact fallback for a symlink-divergent path (e.g. /tmp vs
    // /private/tmp) where the exact prefix missed: match the …/files/<slug>/<rel>
    // layout, requiring the slug segment when we know it. (Legacy artifacts/ is
    // migrated to files/ in place, so it never appears in a live path.)
    const slugPat = slug ? escapeRegExp(slug) : "[^/]+";
    const fd = path.match(new RegExp(`/files/${slugPat}/(.+)$`));
    const wt = fd ? null : path.match(/\/openresearch\/worktrees\/[^/]+\/([^/]+)\/(.+)$/);
    const hub = fd || wt ? null : path.match(/\/openresearch\/repos\/[^/]+\/[^/]+\/(.+)$/);
    if (fd) {
      return { path: fd[1], source: "artifacts" };
    } else if (wt) {
      sessionId = wt[1];
      path = wt[2];
    } else if (hub) {
      path = hub[1];
    }
  }
  if (!path) return null;
  // An absolute path none of the checkout/artifacts branches recognized (e.g.
  // /Users/me/.ssh/config) reads straight off disk — the repo /file endpoint
  // only takes repo-relative paths and would reject it.
  if (path.startsWith("/")) return { path, source: "abs" };
  return { path, sessionId };
}

/** The git branch a code file tab is showing, for the header pill — a cited
 * experiment's branch (or any ref view) names that branch, and a worktree/clone
 * file falls back to the baseline branch, so a code tab always says which
 * branch its contents came from. Artifacts and absolute-path files have no
 * branch. */
function fileBranchLabel(tab: FileViewDef, baselineBranch?: string): string | undefined {
  if (tab.source === "artifacts" || tab.source === "abs") return undefined;
  return tab.ref ?? tab.branchLabel ?? baselineBranch;
}

type ExperimentsView = "tree" | "table";

/** Floating panel sizing: keep both the panel and the chat column usable. */
const PANEL_MIN_WIDTH = 360;
const PANEL_MARGIN = 10;
// Space the rest of the layout needs beside the panel: the 272px rail, the
// chat column's minimum, and the gutters/margins between the three columns
// (app-body padding 14×2, rail inner margin 14, end-pane inner margin 14).
const RAIL_WIDTH = 272;
const CHAT_MIN_SPACE = 380;
const LAYOUT_CHROME = RAIL_WIDTH + 14 * 4;
// Once a drag pushes the panel past its usable max by this much, it snaps to
// fullscreen — a bit of resistance you have to overcome deliberately.
const FULLSCREEN_SNAP_SLOP = 80;
// Inward drag needed before snapping back to the last non-fullscreen width.
const FULLSCREEN_RESTORE_DRAG = 48;

/** The widest the floating panel can be while leaving the rail + chat usable. */
function panelMaxWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, window.innerWidth - LAYOUT_CHROME - CHAT_MIN_SPACE);
}

function initialPanelWidth(): number {
  const max = panelMaxWidth();
  return Math.max(PANEL_MIN_WIDTH, Math.min(760, max, Math.round(window.innerWidth * 0.4)));
}

function upsert<T extends { id: string }>(list: T[], item: T): T[] {
  const i = list.findIndex((x) => x.id === item.id);
  if (i < 0) return [...list, item];
  const next = list.slice();
  next[i] = item;
  return next;
}

function useStableStringMap(next: Map<string, string>): Map<string, string> {
  const current = useRef(next);
  const unchanged = current.current.size === next.size
    && [...next].every(([key, value]) => current.current.get(key) === value);
  if (!unchanged) current.current = next;
  return current.current;
}

export default function App({ runtime, projectId, pane }: { runtime: RuntimeInfo; projectId: string; pane?: Pane }) {
  const router = useRouter();
  const location = useRouterState({ select: (state) => state.location });
  const destination = parseDestination(location.pathname);
  const mainView = destination?.kind === "skills" ? "skills" : destination?.kind === "settings" ? destination.section ?? "settings" : "chat";
  const rememberedSessionRef = useRef<string | null>(null);
  const activeSessionId = destination?.kind === "task" ? destination.sessionId ?? null : rememberedSessionRef.current;
  if (destination?.kind === "task") rememberedSessionRef.current = activeSessionId;
  const panelOpen = pane !== undefined;
  const selectedRunId = pane?.kind === "experiment" ? pane.runId ?? null : null;
  const [consumedLine, setConsumedLine] = useState<number | null>(null);
  const [lineJump, setLineJump] = useState(0);
  const lineVisit = useRef({ href: "", jump: 0, value: 0 });
  if (lineVisit.current.href !== location.href || lineVisit.current.jump !== lineJump) lineVisit.current = { href: location.href, jump: lineJump, value: lineVisit.current.value + 1 };
  const rightTab = useMemo<RightTab>(() => {
    const tab = pane ? paneTab(pane) : "experiments";
    return typeof tab === "object" && "path" in tab && tab.line && consumedLine !== lineVisit.current.value
      ? { ...tab, lineScrollRequest: lineVisit.current.value } : tab;
  }, [pane, consumedLine, location.href, lineJump]);
  const [sessions, setSessions] = useState<string[] | null>(null);
  const restoredFilesRef = useRef(new Set<string>());
  const intentionalFilesRef = useRef(new Set<string>());
  const sourceModesRef = useRef<Record<string, boolean>>({});
  const [metadataRevision, setMetadataRevision] = useState(0);
  const navigationRef = useRef({ projectId, activeSessionId, pane, isTask: destination?.kind === "task" });
  navigationRef.current = { projectId, activeSessionId, pane, isTask: destination?.kind === "task" };
  const navigatePane = useCallback((next: Pane | undefined, replace = false) => {
    const current = navigationRef.current;
    if (current.projectId) void router.navigate({ href: taskLocation(current.projectId, current.activeSessionId, next), replace });
  }, [router]);
  const setRightTab = useCallback((tab: RightTab) => {
    const next = tabPane(tab);
    navigatePane(next.kind === "file" ? { ...next, line: undefined } : next);
  }, [navigatePane]);
  const closePanel = useCallback(() => navigatePane(undefined), [navigatePane]);
  const setSelectedRunId = useCallback((runId: string | null) => {
    if (pane?.kind === "experiment") navigatePane({ ...pane, runId: runId ?? undefined });
  }, [pane, navigatePane]);
  const setProjectId = useCallback((id: string | null) => {
    void router.navigate({ href: id ? `/projects/${encodeURIComponent(id)}` : "/projects" });
  }, [router]);
  const setMainView = useCallback((view: "chat" | "skills" | SettingsTab) => {
    if (!projectId) return;
    if (view === "chat") {
      const id = rememberedSessionRef.current;
      void router.navigate({ href: taskLocation(projectId, id, getTaskWorkspace(workspaceRef.current, id ?? "new")?.active) });
    } else void router.navigate({ href: `/projects/${encodeURIComponent(projectId)}/${view === "skills" ? "skills" : `settings/${view}`}` });
  }, [router, projectId]);

  const locale = useLocale();
  const { status: updateStatus } = useUpdateStatus(runtime.kind === "local");
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [uiState, setUiState] = useState<UiState | null>(null);
  const tourCompletedRef = useRef<boolean | undefined>(undefined);
  tourCompletedRef.current = uiState?.tourCompleted;
  const [startupError, setStartupError] = useState<string | null>(null);
  const persistedPreferredAgent = useRef<AgentSelection | null>(null);
  const [experiments, setExperiments] = useState<Experiment[]>([]);
  const [experimentDataReady, setExperimentDataReady] = useState(false);
  const [runDataReady, setRunDataReady] = useState(false);
  const [runs, setRuns] = useState<Run[]>([]);
  // Latest runs/experiments, read by the stable openRunLogs/openFileTab so
  // evidence chips resolve ids without re-creating the callbacks on every poll
  // (they feed the memoized transcript, which needs stable props).
  const runsRef = useRef(runs);
  runsRef.current = runs;
  // A first-seen running row may be a snapshot; only baseline-new ids or observed edges are live.
  const observedRunsRef = useRef(new Map<string, Run>());
  const liveRunIdsRef = useRef(new Set<string>());
  const observedRunsProjectRef = useRef<string | null>(null);
  const runsBaselineReadyRef = useRef(false);
  const baselineRunsRef = useRef(new Map<string, Run>());
  const pendingFirstRunningRunsRef = useRef(new Map<string, Run>());
  const runsVisitRef = useRef(0);
  const experimentsRef = useRef(experiments);
  experimentsRef.current = experiments;
  const [artifacts, setArtifacts] = useState<ProjectArtifacts | null>(null);
  const [view, setView] = useState<ExperimentsView>("table");
  // Experiments pane scope: "agent" narrows to the open chat session's work.
  // Falls back to "project" whenever there is no usable experiment attribution.
  const [scope, setScope] = useState<"agent" | "project">("project");
  const scopeTriggerRef = useRef<HTMLButtonElement>(null);
  const { open: scopeMenuOpen, setOpen: setScopeMenuOpen, ref: scopeMenuRef } =
    usePopover(scopeTriggerRef);
  const [demoOverviewLeading, setDemoOverviewLeading] = useState(false);
  const allExperimentsAttributed = experiments.every((experiment) => experiment.chatSessionId);
  const effectiveScope = activeSessionId && allExperimentsAttributed ? scope : "project";
  const scopedExperiments = useMemo(() => {
    if (effectiveScope !== "agent") return experiments;
    return experiments.filter((experiment) => experiment.chatSessionId === activeSessionId);
  }, [experiments, effectiveScope, activeSessionId]);
  // Runs are scoped by their experiment's owner, not by which session launched them.
  const scopedRuns = useMemo(() => {
    if (effectiveScope !== "agent") return runs;
    const mine = new Set(scopedExperiments.map((experiment) => experiment.id));
    return runs.filter((r) => mine.has(r.experimentId));
  }, [runs, scopedExperiments, effectiveScope]);

  // Right-panel tab strip: closable home and working tabs. The same experiment
  // can keep both its overview and terminal open.
  const [tabHistory, setTabHistory] = useState<RightTab[]>([]);
  const [experimentsTabOpen, setExperimentsTabOpen] = useState(false);
  const [filesTabOpen, setFilesTabOpen] = useState(false);
  const [artifactsTabOpen, setArtifactsTabOpen] = useState(false);
  const [expTabs, setExpTabs] = useState<ExpViewDef[]>([]);
  const [fileTabs, setFileTabs] = useState<FileViewDef[]>([]);
  const fileScrollPositionsRef = useRef(new Map<string, FileScrollPosition>());
  const fileBuffersRef = useRef(new Map<string, FileBufferSession>());
  const getFileBufferSession = (key: string) => {
    let buffer = fileBuffersRef.current.get(key);
    if (!buffer) {
      buffer = new FileBufferSession();
      fileBuffersRef.current.set(key, buffer);
    }
    return buffer;
  };
  const [planTabs, setPlanTabs] = useState<PlanViewDef[]>([]);
  const [subagentTabs, setSubagentTabs] = useState<SubagentViewDef[]>([]);
  const [codeTabs, setCodeTabs] = useState<CodeTabDef[]>([]);
  const [contentTabOrder, setContentTabOrderState] = useState<string[]>([]);
  const [previewTab, setPreviewTabState] = useState<RightTab | null>(null);
  const [filesView, setFilesView] = useState<WorktreeView>("files");
  const [filesToggled, setFilesToggled] = useState<ReadonlySet<string>>(new Set());
  // The right pane is a floating panel: closable, edge-resizable, expandable
  // to (nearly) full screen. Width persists across sessions.
  const [panelMax, setPanelMax] = useState(false);
  const [panelWidth, setPanelWidth] = useState(initialPanelWidth);
  // The agents rail is a floating panel too: fixed-width, collapsible.
  const [railOpen, setRailOpen] = useState(true);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const currentRightPaneStateRef = useRef<RightPaneSessionState>(initialRightPaneSessionState());
  const activeSessionIdRef = useRef<string | null>(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  const tabHistoryRef = useRef(tabHistory);
  tabHistoryRef.current = tabHistory;
  const contentTabOrderRef = useRef(contentTabOrder);
  contentTabOrderRef.current = contentTabOrder;
  const previewTabRef = useRef<RightTab | null>(null);

  const setContentTabOrder = useCallback((order: readonly string[]) => {
    const next = [...order];
    contentTabOrderRef.current = next;
    setContentTabOrderState(next);
  }, []);

  const setPreviewTab = useCallback((tab: RightTab | null) => {
    previewTabRef.current = tab;
    setPreviewTabState(tab);
  }, []);

  // Retire content replaced out of the reusable preview slot. The caller owns
  // the slot order because the replacement must land at this exact position.
  const retireRightTab = useCallback((tab: ContentTab) => {
    const key = rightTabKey(tab);
    setExpTabs((prev) => withoutTab(prev, key));
    setFileTabs((prev) => withoutTab(prev, key));
    setPlanTabs((prev) => withoutTab(prev, key));
    setSubagentTabs((prev) => withoutTab(prev, key));
    setCodeTabs((prev) => withoutTab(prev, key));
    const project = projectIdRef.current;
    if (project && "path" in tab) {
      const fileKey = fileScrollKey(project, activeSessionIdRef.current, tab);
      fileScrollPositionsRef.current.delete(fileKey);
      fileBuffersRef.current.delete(fileKey);
      intentionalFilesRef.current.delete(fileKey);
      restoredFilesRef.current.delete(fileKey);
    }
    const next = tabHistoryRef.current.filter((item) => rightTabKey(item) !== key);
    tabHistoryRef.current = next;
    setTabHistory(next);
  }, []);

  const selectRightTab = useCallback((tab: RightTab) => {
    const key = rightTabKey(tab);
    const next = [
      ...tabHistoryRef.current.filter((item) => rightTabKey(item) !== key),
      persistentRightTab(tab),
    ];
    tabHistoryRef.current = next;
    setTabHistory(next);
    setRightTab(tab);
  }, [setRightTab]);

  const openRightTab = useCallback((tab: ContentTab, intent: TabOpenIntent, runId?: string) => {
    const key = rightTabKey(tab);
    const outgoing = previewTabRef.current;
    const transition = openTab(
      {
        order: contentTabOrderRef.current,
        previewKey: outgoing ? rightTabKey(outgoing) : null,
      },
      key,
      intent,
    );
    if (
      transition.replacedKey &&
      outgoing &&
      typeof outgoing !== "string" &&
      rightTabKey(outgoing) === transition.replacedKey
    ) {
      retireRightTab(outgoing);
    }
    setContentTabOrder(transition.order);
    if (transition.previewKey === null) setPreviewTab(null);
    else if (transition.previewKey === key) setPreviewTab(persistentRightTab(tab));

    const next = [
      ...tabHistoryRef.current.filter((item) => rightTabKey(item) !== key),
      persistentRightTab(tab),
    ];
    tabHistoryRef.current = next;
    setTabHistory(next);
    navigatePane(tabPane(tab, runId));
  }, [retireRightTab, setContentTabOrder, setPreviewTab, navigatePane]);

  const promoteRightTab = useCallback((tab: RightTab) => {
    const current = previewTabRef.current;
    if (current && rightTabKey(current) === rightTabKey(tab)) setPreviewTab(null);
  }, [setPreviewTab]);

  useEffect(() => {
    let waitingForEnter = false;
    const onKeyDown = (event: KeyboardEvent) => {
      const preview = previewTabRef.current;
      const target = event.target;
      const editing =
        target instanceof Element &&
        target.closest("input, textarea, [contenteditable='true']") !== null;
      if (editing) {
        waitingForEnter = false;
        return;
      }
      if (
        preview &&
        rightTabKey(preview) === rightTabKey(currentRightPaneStateRef.current.rightTab) &&
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "k"
      ) {
        event.preventDefault();
        waitingForEnter = true;
        return;
      }
      if (waitingForEnter && event.key === "Enter") {
        event.preventDefault();
        waitingForEnter = false;
        const activePreview = previewTabRef.current;
        if (activePreview) promoteRightTab(activePreview);
        return;
      }
      waitingForEnter = false;
    };
    const resetChord = () => {
      waitingForEnter = false;
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("blur", resetChord);
    window.addEventListener("pointerdown", resetChord);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("blur", resetChord);
      window.removeEventListener("pointerdown", resetChord);
    };
  }, [promoteRightTab]);

  const forgetRightTab = useCallback((tab: RightTab, selectFallback: boolean) => {
    const key = rightTabKey(tab);
    const preview = previewTabRef.current;
    if (preview && rightTabKey(preview) === key) setPreviewTab(null);
    const contentState = closeTab(
      {
        order: contentTabOrderRef.current,
        previewKey: preview ? rightTabKey(preview) : null,
      },
      key,
      tabHistoryRef.current.map(rightTabKey),
    );
    setContentTabOrder(contentState.order);
    const next = tabHistoryRef.current.filter((item) => rightTabKey(item) !== key);
    tabHistoryRef.current = next;
    setTabHistory(next);
    if (!selectFallback) return;
    const fallback = contentState.fallbackKey
      ? next.find((item) => rightTabKey(item) === contentState.fallbackKey)
      : undefined;
    if (fallback) setRightTab(fallback);
    else {
      closePanel();
      setPanelMax(false);
    }
  }, [setContentTabOrder, setPreviewTab, setRightTab, closePanel]);

  const selectMainView = setMainView;

  const rightPaneState = useMemo<RightPaneSessionState>(() => ({
    rightTab: persistentRightTab(rightTab),
    tabHistory,
    experimentsTabOpen,
    filesTabOpen,
    artifactsTabOpen,
    expTabs,
    fileTabs,
    planTabs,
    subagentTabs,
    codeTabs,
    contentTabOrder: contentTabOrderRef.current,
    previewTab: previewTabRef.current,
    filesView,
    filesToggled,
    selectedRunId,
    scope,
    panelOpen,
    panelMax,
  }), [rightTab, tabHistory, experimentsTabOpen, filesTabOpen, artifactsTabOpen, expTabs, fileTabs, planTabs, subagentTabs, codeTabs, contentTabOrder, previewTab, filesView, filesToggled, selectedRunId, scope, panelOpen, panelMax]);
  currentRightPaneStateRef.current = rightPaneState;
  const getFileScroll = useCallback(() => Object.fromEntries(fileScrollPositionsRef.current), []);
  const scrollSaveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const recordScroll = useCallback(() => {
    clearTimeout(scrollSaveTimer.current);
    scrollSaveTimer.current = setTimeout(() => setMetadataRevision((value) => value + 1), 200);
  }, []);
  useEffect(() => () => clearTimeout(scrollSaveTimer.current), []);
  const applyWorkspace = useCallback((state: RightPaneSessionState, saved: TaskWorkspace | undefined, restored: boolean) => {
    setTabHistory(state.tabHistory);
    tabHistoryRef.current = state.tabHistory;
    setExperimentsTabOpen(state.experimentsTabOpen);
    setFilesTabOpen(state.filesTabOpen);
    setArtifactsTabOpen(state.artifactsTabOpen);
    setExpTabs(state.expTabs);
    setFileTabs(state.fileTabs);
    setPlanTabs((current) => current === state.planTabs ? current : state.planTabs.map((tab) => ({ ...tab, plan: current.find((item) => item.sessionId === tab.sessionId && item.promptId === tab.promptId)?.plan ?? "" })));
    setSubagentTabs(state.subagentTabs);
    setCodeTabs(state.codeTabs);
    setContentTabOrder(state.contentTabOrder);
    setPreviewTab(state.previewTab);
    setFilesView(state.filesView);
    setFilesToggled(state.filesToggled);
    setScope(state.scope);
    setPanelMax(state.panelMax);
    setDemoOverviewLeading(navigationRef.current.activeSessionId === DEMO_MAIN_SESSION_ID && state.fileTabs.some((tab) => sameFileTab(tab, { path: DEMO_OVERVIEW_ARTIFACT, source: "artifacts" })));
    if (restored) {
      for (const [key, position] of Object.entries(saved?.scroll ?? {})) fileScrollPositionsRef.current.set(key, position);
      Object.assign(sourceModesRef.current, saved?.sourceModes);
    }
    const current = navigationRef.current;
    if (current.projectId) for (const tab of state.fileTabs) {
      const key = fileScrollKey(current.projectId, current.activeSessionId, tab);
      if (!intentionalFilesRef.current.has(key)) restoredFilesRef.current.add(key);
    }
  }, [setContentTabOrder, setPreviewTab]);
  const { ready: workspaceReady, loaded: workspaceLoaded, error: workspaceError, retry: retryWorkspace, capture: captureWorkspace, workspace: workspaceRef } = useProjectWorkspace({
    projectId: uiState && sessions !== null && (destination?.kind !== "task" || !activeSessionId || sessions.includes(activeSessionId)) ? projectId : null, taskKey: activeSessionId ?? "new", location: location.href, pane,
    isTask: destination?.kind === "task", demoOverview: uiState?.tourCompleted === false, state: rightPaneState,
    apply: applyWorkspace, getScroll: getFileScroll,
    sourceModes: sourceModesRef.current, revision: metadataRevision,
  });
  const onActiveSessionChange = useCallback((sessionId: string | null, options?: { replace?: boolean }) => {
    if (!projectId) return;
    if (sessionId) {
      sessionLoadRef.current?.set(sessionId, true);
      setSessions((current) => current && current.includes(sessionId) ? current : [...(current ?? []), sessionId]);
      if (options?.replace && activeSessionId === null) {
        captureWorkspace();
        inheritNewTaskWorkspace(projectId, sessionId);
      }
    }
    const saved = getTaskWorkspace(getCachedProjectWorkspace(projectId), sessionId ?? "new");
    const remembered = saved ? saved.active : (isDemoProjectId(projectId) ? defaultTaskWorkspace(sessionId ?? undefined, tourCompletedRef.current === false)?.active : undefined);
    const nextPane = options?.replace && sessionId && activeSessionId === null ? navigationRef.current.pane : remembered;
    void router.navigate({ href: taskLocation(projectId, sessionId, nextPane), replace: options?.replace });
  }, [router, projectId, activeSessionId, captureWorkspace]);
  useEffect(() => {
    if (workspaceReady && destination?.kind !== "task" && !rememberedSessionRef.current && workspaceRef.current.lastTaskId && sessions?.includes(workspaceRef.current.lastTaskId)) {
      rememberedSessionRef.current = workspaceRef.current.lastTaskId;
      setMetadataRevision((value) => value + 1);
    }
  }, [workspaceReady, destination?.kind, workspaceRef, sessions]);
  useBlocker({
    shouldBlockFn: ({ next }) => parseDestination(next.pathname)?.projectId !== projectId
      && [...fileBuffersRef.current.values()].some((buffer) => buffer.needsProtection) && !confirmFileDiscard(m.file_viewer_discard_unsaved_changes()),
    enableBeforeUnload: () => [...fileBuffersRef.current.values()].some((buffer) => buffer.needsProtection),
  });
  const lastGlobalLocation = useRef<string | null>(null);
  useEffect(() => {
    const savedLocation = safeLocation(location.href);
    if (!uiState || !workspaceReady || !savedLocation) return;
    globalWorkspaceWriter.queue({ lastLocation: savedLocation, railOpen, panelWidth, experimentsView: view }, lastGlobalLocation.current === savedLocation ? 250 : 0);
    lastGlobalLocation.current = savedLocation;
  }, [location.href, uiState, workspaceReady, railOpen, panelWidth, view]);
  const onboarded = uiState?.onboardingCompleted ?? false;
  const [demoWelcomeOpen, setDemoWelcomeOpen] = useState(false);
  const openDemoWelcome = useCallback(() => setDemoWelcomeOpen(true), []);
  const closeDemoWelcome = useCallback(async () => {
    const saved = await updateUiState({ tourCompleted: true });
    setUiState((current) => current && { ...current, tourCompleted: saved.tourCompleted });
    setDemoWelcomeOpen(false);
  }, []);
  const createProjectFromDemoWelcome = useCallback(async () => {
    await closeDemoWelcome();
    setNewProjectOpen(true);
  }, [closeDemoWelcome]);

  // Show the welcome once the bundled demo is first visible. User projects
  // never open it automatically.
  useEffect(() => {
    if (!projectId || !isDemoProjectId(projectId) || !onboarded) return;
    if (uiState?.tourCompleted) return;
    openDemoWelcome();
  }, [projectId, onboarded, openDemoWelcome, uiState?.tourCompleted]);

  const activeProject = projects?.find((p) => p.id === projectId) ?? null;

  // The home, error, and loading screens leave projects populated but show no project.
  useEffect(() => {
    const name = startupError || uiState === null ? null : activeProject?.name;
    document.title = name ? `${autoDir(name)} — OpenResearch` : "OpenResearch";
  }, [startupError, uiState, activeProject]);

  const projectIdRef = useRef(projectId);
  projectIdRef.current = projectId;

  const openExperimentsTab = useCallback((replace = false) => {
    if (replace && (!navigationRef.current.isTask || navigationRef.current.pane)) return;
    setExperimentsTabOpen(true);
    navigatePane({ kind: "home", view: "experiments" }, replace);
  }, [navigatePane]);

  const sessionLoadRef = useRef<Map<string, boolean> | null>(null);
  const loadSessionIds = useCallback(async () => {
    const changes = new Map<string, boolean>();
    sessionLoadRef.current = changes;
    const loaded = await listChatSessions(projectId);
    if (sessionLoadRef.current !== changes) return;
    const ids = new Set(loaded.map((session) => session.id));
    for (const [id, present] of changes) {
      if (present) ids.add(id);
      else ids.delete(id);
    }
    sessionLoadRef.current = null;
    setSessions([...ids]);
    if (rememberedSessionRef.current && !ids.has(rememberedSessionRef.current)) rememberedSessionRef.current = null;
  }, [projectId]);

  const loadInitialState = useCallback(() => {
    setStartupError(null);
    setProjects(null);
    setUiState(null);
    void Promise.allSettled([listProjects(), getUiState(), loadSessionIds()]).then(([projectsResult, uiStateResult, sessionsResult]) => {
      const errors: string[] = [];
      if (sessionsResult.status === "rejected") errors.push(m.chat_all_sessions());
      if (projectsResult.status === "fulfilled") {
        setProjects(projectsResult.value);

      } else {
        errors.push(m.app_projects());
      }
      if (uiStateResult.status === "fulfilled") {
        persistedPreferredAgent.current = uiStateResult.value.preferredAgent;
        const prefs = getRememberedGlobalWorkspace() ?? uiStateResult.value.workspace;
        if (prefs) {
          setRailOpen(prefs.railOpen);
          setPanelWidth(Math.min(prefs.panelWidth, panelMaxWidth()));
          setView(prefs.experimentsView);
        }
        setUiState(uiStateResult.value);
      } else {
        errors.push(m.app_settings());
      }
      if (errors.length > 0) {
        setStartupError(m.app_startup_load_failed({ items: new Intl.ListFormat(getLocale()).format(errors) }));
      }
    });
  }, [loadSessionIds]);
  useEffect(() => {
    loadInitialState();
  }, [loadInitialState]);

  useEffect(() => {
    const off = onChatEvent((event) => {
      if (event.type === "reconnected") {
        void loadSessionIds().catch(() => {});
      } else if (event.type === "session" && event.session.projectId === projectId) {
        sessionLoadRef.current?.set(event.session.id, true);
        setSessions((current) => current?.includes(event.session.id) ? current : [...(current ?? []), event.session.id]);
      } else if (event.type === "sessionDeleted") {
        sessionLoadRef.current?.set(event.sessionId, false);
        setSessions((current) => current?.filter((id) => id !== event.sessionId) ?? null);
        if (rememberedSessionRef.current === event.sessionId) rememberedSessionRef.current = null;
      }
    });
    return () => { off(); sessionLoadRef.current = null; };
  }, [projectId, loadSessionIds]);

  const preferredAgentWrite = useRef<Promise<void>>(Promise.resolve());
  const preferredAgentSaveSeq = useRef(0);
  const persistPreferredAgent = useCallback((selection: AgentSelection) => {
    const saveSeq = ++preferredAgentSaveSeq.current;
    setUiState((current) => current && { ...current, preferredAgent: selection });
    const write = preferredAgentWrite.current
      .then(() => updateUiState({ preferredAgent: selection }))
      .then((saved) => {
        persistedPreferredAgent.current = saved.preferredAgent;
        if (saveSeq === preferredAgentSaveSeq.current) {
          setUiState((current) => current && { ...current, preferredAgent: saved.preferredAgent });
        }
      })
      .catch((error: unknown) => {
        if (saveSeq === preferredAgentSaveSeq.current) {
          setUiState((current) =>
            current && { ...current, preferredAgent: persistedPreferredAgent.current },
          );
        }
        throw error;
      });
    preferredAgentWrite.current = write.catch(() => {});
    return write;
  }, []);

  // Shrinking the window can push a fixed-width panel past its usable max —
  // reclamp so it never overflows the viewport.
  useEffect(() => {
    const onResize = () => setPanelWidth((w) => Math.min(w, panelMaxWidth()));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const loadRunsBaseline = useCallback((baselineProjectId: string) => {
    runsBaselineReadyRef.current = false;
    baselineRunsRef.current.clear();
    pendingFirstRunningRunsRef.current.clear();
    const runsVisit = ++runsVisitRef.current;
    listRuns(baselineProjectId)
      .then((loadedRuns) => {
        if (
          projectIdRef.current !== baselineProjectId ||
          observedRunsProjectRef.current !== baselineProjectId ||
          runsVisitRef.current !== runsVisit
        ) return;
        baselineRunsRef.current = new Map(loadedRuns.map((run) => [run.id, run]));
        const shouldAutoOpen = [...pendingFirstRunningRunsRef.current.values()].some(
          (liveRun) => {
            const baselineRun = baselineRunsRef.current.get(liveRun.id);
            return (
              !baselineRun ||
              (baselineRun.status !== "running" && baselineRun.updatedAt <= liveRun.updatedAt)
            );
          },
        );
        pendingFirstRunningRunsRef.current.clear();
        for (const run of loadedRuns) {
          const observed = observedRunsRef.current.get(run.id);
          if (!observed || observed.updatedAt < run.updatedAt) {
            observedRunsRef.current.set(run.id, run);
          }
        }
        setRuns((current) => {
          const merged = new Map(loadedRuns.map((run) => [run.id, run]));
          for (const liveRun of current) {
            const fetchedRun = merged.get(liveRun.id);
            if (!fetchedRun || fetchedRun.updatedAt <= liveRun.updatedAt) {
              merged.set(liveRun.id, liveRun);
            }
          }
          return [...merged.values()];
        });
        runsBaselineReadyRef.current = true;
        setRunDataReady(true);
        if (shouldAutoOpen) openExperimentsTab(true);
      })
      .catch(() => {
        if (runsVisitRef.current === runsVisit) {
          pendingFirstRunningRunsRef.current.clear();
          setRunDataReady(true);
        }
      });
  }, [openExperimentsTab]);

  // Per-project data. Harness agents spawn lazily on the first chat message.
  useEffect(() => {
    if (!projectId) return;
    let active = true;
    observedRunsProjectRef.current = projectId;
    observedRunsRef.current.clear();
    liveRunIdsRef.current.clear();
    openProject(projectId).catch(() => {});
    setExperiments([]);
    setRuns([]);
    setArtifacts(null);
    listExperiments(projectId).then((items) => { if (active) { setExperiments(items); setExperimentDataReady(true); } }).catch(() => { if (active) setExperimentDataReady(true); });
    loadRunsBaseline(projectId);
    getArtifacts(projectId).then((items) => { if (active) setArtifacts(items); }).catch(() => {});
    return () => { active = false; runsVisitRef.current++; };
  }, [loadRunsBaseline, projectId]);

  // Refetch artifacts on open and whenever the directory changes.
  const refreshArtifacts = useCallback(() => {
    const id = projectIdRef.current;
    if (id) getArtifacts(id).then(setArtifacts).catch(() => {});
  }, []);

  const openArtifactsTab = useCallback(() => {
    refreshArtifacts();
    setArtifactsTabOpen(true);
    selectRightTab("artifacts");
  }, [refreshArtifacts, selectRightTab]);

  // Live store updates.
  useOrxEvents({
    onReconnect: () => {
      const id = projectIdRef.current;
      if (!id) return;
      observedRunsProjectRef.current = id;
      observedRunsRef.current.clear();
      liveRunIdsRef.current.clear();
      loadRunsBaseline(id);
    },
    onRun: (run) => {
      if (
        run.projectId !== projectIdRef.current ||
        run.projectId !== observedRunsProjectRef.current
      ) return;
      const previous = observedRunsRef.current.get(run.id);
      const previouslyLive = liveRunIdsRef.current.has(run.id);
      if (previous && previous.updatedAt > run.updatedAt) return;
      observedRunsRef.current.set(run.id, run);
      liveRunIdsRef.current.add(run.id);
      setRuns((current) => upsert(current, run));
      if (run.status !== "running" || previous?.status === "running") return;
      const baselineRun = baselineRunsRef.current.get(run.id);
      const newSinceBaseline =
        runsBaselineReadyRef.current &&
        (!baselineRun ||
          (baselineRun.status !== "running" && baselineRun.updatedAt <= run.updatedAt));
      if ((previouslyLive && previous) || newSinceBaseline) openExperimentsTab(true);
      else if (!runsBaselineReadyRef.current) pendingFirstRunningRunsRef.current.set(run.id, run);
    },
    onExperiment: (experiment) => {
      if (experiment.projectId === projectIdRef.current)
        setExperiments((cur) => upsert(cur, experiment));
    },
    onProject: (project) => {
      setProjects((cur) => (cur ? upsert(cur, project) : [project]));
    },
    onArtifacts: (pid) => {
      if (pid === projectIdRef.current) refreshArtifacts();
    },
  });

  // Stable identity: in TreeView's layout-memo deps, so an inline arrow would
  // recompute the graph on every render.
  const showProjectScope = useCallback(() => setScope("project"), []);

  // Open an experiment view as a right-panel tab (creating it if needed) and
  // focus it.
  const openExperimentTab = useCallback((
    id: string,
    view: ExperimentView = "overview",
    intent: TabOpenIntent = "preview",
    runId?: string,
  ) => {
    const tab = { id, view };
    setExpTabs((prev) => (prev.some((t) => sameExpTab(t, tab)) ? prev : [...prev, tab]));
    openRightTab(tab, intent, runId);
  }, [openRightTab]);

  // A `<run>` evidence chip in chat opens that run's logs — the only evidence
  // channel for a metric. Run ids are globally unique, so resolve the run to its
  // experiment and open the terminal view focused on it.
  const openRunLogs = useCallback(
    (runId: string, intent: TabOpenIntent = "preview") => {
      const matches = runsRef.current.filter((run) => run.id === runId || run.id.startsWith(runId));
      const run = matches.length === 1 ? matches[0] : null;
      if (!run) return;
      openExperimentTab(run.experimentId, "terminal", intent, run.id);
    },
    [openExperimentTab],
  );

  const nextExperimentNames = useMemo(
    () => new Map(experiments.map((experiment) => [experiment.id, experiment.title?.trim() || experiment.slug || m.tree_experiment()])),
    [experiments, locale],
  );
  const experimentNames = useStableStringMap(nextExperimentNames);
  const nextRunExperimentNames = useMemo(() => {
    const names = new Map<string, string>();
    for (const run of runs) {
      names.set(run.id, experimentNames.get(run.experimentId) ?? m.tree_experiment());
    }
    return names;
  }, [experimentNames, runs, locale]);
  const runExperimentNames = useStableStringMap(nextRunExperimentNames);

  const runExperimentName = useCallback((runId: string) => {
    const exact = runExperimentNames.get(runId);
    if (exact) return exact;
    const matches = [...runExperimentNames].filter(([id]) => id.startsWith(runId));
    return matches.length === 1 ? matches[0][1] : "";
  }, [runExperimentNames]);

  const experimentName = useCallback((experimentId: string) => {
    const exact = experimentNames.get(experimentId);
    if (exact) return exact;
    const matches = [...experimentNames].filter(([id]) => id.startsWith(experimentId));
    return matches.length === 1 ? matches[0][1] : "";
  }, [experimentNames]);

  const openExperimentNotes = useCallback((
    experimentId: string,
    intent: TabOpenIntent = "preview",
  ) => {
    const matches = experimentsRef.current.filter(
      (experiment) => experiment.id === experimentId || experiment.id.startsWith(experimentId),
    );
    if (matches.length === 1) openExperimentTab(matches[0].id, "overview", intent);
  }, [openExperimentTab]);

  const closeExperimentTab = useCallback(
    (tab: ExpViewDef) => {
      const idx = expTabs.findIndex((t) => sameExpTab(t, tab));
      if (idx === -1) return;
      setExpTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [expTabs, forgetRightTab, rightTab],
  );

  const openResolvedFileTab = useCallback(
    (tab: FileViewDef, intent: TabOpenIntent = "preview") => {
      if (tab.line != null) setLineJump((value) => value + 1);
      const project = projectIdRef.current;
      if (project) {
        const key = fileScrollKey(project, activeSessionIdRef.current, tab);
        if (!currentRightPaneStateRef.current.fileTabs.some((item) => sameFileTab(item, tab))) restoredFilesRef.current.delete(key);
        intentionalFilesRef.current.add(key);
      }
      const persistentTab = persistentFileTab(tab);
      setFileTabs((prev) => {
        const idx = prev.findIndex((item) => sameFileTab(item, tab));
        if (idx === -1) return [...prev, persistentTab];
        const next = prev.slice();
        next[idx] = persistentTab;
        return next;
      });
      openRightTab(tab, intent);
    },
    [openRightTab],
  );

  // Resolve a reported path to a file tab. `contextSessionId` is the chat
  // session (or viewed file's session) the click came from — see
  // parseFilePath for how it resolves against the reported path.
  const resolveFileTab = useCallback(
    (
      rawPath: string,
      contextSessionId?: string,
      ref?: string,
      line?: number,
      exp?: string,
      displayBranch?: string,
    ) => {
      const project = projects?.find((p) => p.id === projectId);
      const tab = parseFilePath(
        rawPath,
        project?.repoPath,
        contextSessionId,
        project?.artifactsDir ?? project?.filesDir,
        project?.slug,
      );
      if (!tab) return null;
      // A cited experiment pins the file to that node's committed branch, so the
      // tab shows (and labels) the version behind the claim. Agents cite the
      // short id (`orx` prints an 8-char prefix), so match the full id or prefix.
      const experiment = exp
        ? experimentsRef.current.find(
            (e) => e.id === exp || (exp.length >= 6 && e.id.startsWith(exp)),
          )
        : undefined;
      const effectiveRef = ref ?? experiment?.branchName;
      // A branch ref only applies to repo files; artifacts and absolute-path
      // files have no branch.
      const isRepoFile = tab.source == null || tab.source === "repo";
      if (effectiveRef && isRepoFile) tab.ref = effectiveRef;
      // Label an editable (ref-less) checkout with its branch, so a worktree on
      // a non-baseline branch still names it in the header.
      if (displayBranch && !tab.ref && isRepoFile) tab.branchLabel = displayBranch;
      // Line is not part of tab identity: reopening a file at a new line reuses
      // the tab but makes the new (line-carrying) def the active one so the
      // viewer re-scrolls.
      if (line != null) {
        tab.line = line;
      }
      return tab;
    },
    [projects, projectId],
  );

  const openFileTab = useCallback(
    (
      rawPath: string,
      contextSessionId?: string,
      ref?: string,
      line?: number,
      exp?: string,
      displayBranch?: string,
      intent: TabOpenIntent = "preview",
    ) => {
      const tab = resolveFileTab(rawPath, contextSessionId, ref, line, exp, displayBranch);
      if (tab) openResolvedFileTab(tab, intent);
    },
    [openResolvedFileTab, resolveFileTab],
  );

  const openArtifactFileTab = useCallback(
    (path: string) => openResolvedFileTab({ path, source: "artifacts" }, "keepOpen"),
    [openResolvedFileTab],
  );

  // Chat file chips may carry a target line, cited experiment, or an exact Git
  // ref from a `git show ref:path` tool call.
  const openChatFile = useCallback(
    (
      path: string,
      sessionId?: string,
      line?: number,
      exp?: string,
      ref?: string,
      intent: TabOpenIntent = "preview",
    ) => {
      const tab = resolveFileTab(path, sessionId, ref, line, exp);
      if (tab) openResolvedFileTab(tab, intent);
    },
    [openResolvedFileTab, resolveFileTab],
  );

  // Following a chip or link out of a preview keeps the source open before the
  // destination uses its own preview/keep-open intent.
  const openFromRightTab = useCallback(
    (host: RightTab, open: () => void) => {
      promoteRightTab(host);
      open();
    },
    [promoteRightTab],
  );

  const closeFileTab = useCallback(
    (tab: FileViewDef) => {
      const idx = fileTabs.findIndex((t) => sameFileTab(t, tab));
      if (idx === -1) return;
      const key = projectId ? fileScrollKey(projectId, activeSessionId, tab) : null;
      if (key && fileBuffersRef.current.get(key)?.needsProtection && !confirmFileDiscard(m.file_viewer_discard_unsaved_changes())) return;
      setFileTabs((prev) => prev.filter((_, i) => i !== idx));
      if (key) {
        fileScrollPositionsRef.current.delete(key);
        fileBuffersRef.current.delete(key);
        intentionalFilesRef.current.delete(key);
        restoredFilesRef.current.delete(key);
        delete sourceModesRef.current[key];
      }
      if (
        activeSessionId === DEMO_MAIN_SESSION_ID &&
        sameFileTab(tab, { path: DEMO_OVERVIEW_ARTIFACT, source: "artifacts" })
      ) {
        setDemoOverviewLeading(false);
      }
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [activeSessionId, fileTabs, forgetRightTab, projectId, rightTab],
  );

  const consumeFileLineScrollRequest = useCallback((tab: FileViewDef) => {
    if (tab.lineScrollRequest === undefined) return;
    setConsumedLine(tab.lineScrollRequest);
  }, []);

  // Open a proposed plan as a right-panel tab (the chat plan strip's "View
  // plan"). One tab per plan card; re-opening the same card refreshes its
  // text (a revised plan re-uses the strip but is a new promptId → new tab).
  const openPlanTab = useCallback((
    plan: string,
    sessionId: string,
    promptId: string,
    intent: TabOpenIntent = "preview",
  ) => {
    const tab: PlanViewDef = { kind: "plan", sessionId, promptId, plan };
    setPlanTabs((prev) => {
      const idx = prev.findIndex((t) => t.promptId === promptId);
      if (idx === -1) return [...prev, tab];
      const next = prev.slice();
      next[idx] = tab;
      return next;
    });
    openRightTab(tab, intent);
  }, [openRightTab]);

  const closePlanTab = useCallback(
    (tab: PlanViewDef) => {
      const idx = planTabs.findIndex((t) => t.promptId === tab.promptId);
      if (idx === -1) return;
      setPlanTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [forgetRightTab, planTabs, rightTab],
  );

  // Open a sub-agent's transcript as a right-panel tab (a chat spawn row's
  // "view"). One tab per spawn part; its parts stream live off the chat message,
  // so the tab body just reads the current part and needs no fetch.
  const openSubagentTab = useCallback((
    sessionId: string,
    spawnPartId: string,
    label?: string,
    intent: TabOpenIntent = "preview",
  ) => {
    const tab: SubagentViewDef = { kind: "subagent", sessionId, spawnPartId, label };
    setSubagentTabs((prev) =>
      prev.some((t) => t.spawnPartId === spawnPartId) ? prev : [...prev, tab],
    );
    openRightTab(tab, intent);
  }, [openRightTab]);

  const closeSubagentTab = useCallback(
    (tab: SubagentViewDef) => {
      const idx = subagentTabs.findIndex((t) => t.spawnPartId === tab.spawnPartId);
      if (idx === -1) return;
      setSubagentTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [forgetRightTab, rightTab, subagentTabs],
  );

  // Live title + running state for open sub-agent tabs, straight off the spawn
  // parts' message stream — so a tab is named for its task and shimmers while
  // the agent still works (the open-time `label` is only the seed/fallback).
  const [spawnMeta, setSpawnMeta] = useState<Record<string, { label: string; running: boolean }>>({});
  useEffect(() => {
    // Closed tabs drop their metadata — the map only ever holds open tabs.
    setSpawnMeta((prev) => {
      const open = new Set(subagentTabs.map((t) => t.spawnPartId));
      if (Object.keys(prev).every((id) => open.has(id))) return prev;
      return Object.fromEntries(Object.entries(prev).filter(([id]) => open.has(id)));
    });
    if (subagentTabs.length === 0) return;
    let live = true;
    // Spawn ids a live event already updated: the initial fetch can resolve
    // AFTER newer stream frames and must not roll those tabs back (a stale
    // `running` snapshot would shimmer forever).
    const liveUpdated = new Set<string>();
    const apply = (msgs: ChatMessage[], tabs: SubagentViewDef[], fromSeed: boolean) => {
      setSpawnMeta((prev) => {
        let next = prev;
        for (const t of tabs) {
          if (fromSeed && liveUpdated.has(t.spawnPartId)) continue;
          for (const m of msgs) {
            const part = findPartById(m.parts, t.spawnPartId);
            if (!part) continue;
            if (!fromSeed) liveUpdated.add(t.spawnPartId);
            const meta = { label: spawnRowTitle(part), running: part.state?.status === "running" };
            const cur = next[t.spawnPartId];
            if (!cur || cur.label !== meta.label || cur.running !== meta.running) {
              if (next === prev) next = { ...prev };
              next[t.spawnPartId] = meta;
            }
            break;
          }
        }
        return next;
      });
    };
    // Generation token: a reconnect starts fresh seeds, and a stale in-flight
    // response from an earlier generation must not land after them.
    let seedGen = 0;
    const seed = () => {
      const gen = ++seedGen;
      for (const sid of new Set(subagentTabs.map((t) => t.sessionId))) {
        getChatMessages(sid)
          .then(({ messages }) => {
            if (live && gen === seedGen)
              apply(messages, subagentTabs.filter((t) => t.sessionId === sid), true);
          })
          .catch(() => {});
      }
    };
    seed();
    const off = onChatEvent((ev) => {
      if (ev.type === "reconnected") {
        // Frames lost during the outage may include the terminal update —
        // refetch, letting the fresh seed overwrite everything.
        liveUpdated.clear();
        seed();
        return;
      }
      if (ev.type !== "message") return;
      const tabs = subagentTabs.filter((t) => t.sessionId === ev.sessionId);
      if (tabs.length) apply([ev.message], tabs, false);
    });
    return () => {
      live = false;
      off();
    };
  }, [subagentTabs]);

  // One Git-backed code tab per branch. Reopening the same branch focuses it
  // at the requested subview; another branch gets its own tab.
  const openCodeTabForExperiment = useCallback(
    (
      experimentId: string,
      branch: string,
      view: CodeView = "files",
      intent: TabOpenIntent = "preview",
    ) => {
      const opened: CodeTabDef = {
        code: true,
        experimentId,
        branch,
        view,
        toggled: new Set<string>(),
      };
      setCodeTabs((prev) =>
        prev.some((tab) => sameCodeTab(tab, opened))
          ? prev.map((tab) =>
              sameCodeTab(tab, opened) ? { ...tab, experimentId, view } : tab,
            )
          : [...prev, opened],
      );
      openRightTab(opened, intent);
    },
    [openRightTab],
  );

  const updateCodeTab = useCallback(
    (tab: CodeTabDef, patch: Partial<Omit<CodeTabDef, "code" | "branch">>) => {
      setCodeTabs((prev) =>
        prev.map((item) => (sameCodeTab(item, tab) ? { ...item, ...patch } : item)),
      );
      if (patch.view) navigatePane(tabPane({ ...tab, ...patch }));
    },
    [],
  );

  const closeCodeTab = useCallback(
    (tab: CodeTabDef) => {
      const idx = codeTabs.findIndex((item) => sameCodeTab(item, tab));
      if (idx === -1) return;
      setCodeTabs((prev) => prev.filter((_, index) => index !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [codeTabs, forgetRightTab, rightTab],
  );

  const openWorktreeTab = useCallback(() => {
    setFilesTabOpen(true);
    selectRightTab("files");
  }, [selectRightTab]);

  const closeHomeTab = useCallback(
    (tab: "experiments" | "files" | "artifacts") => {
      if (tab === "experiments") setExperimentsTabOpen(false);
      else if (tab === "files") setFilesTabOpen(false);
      else setArtifactsTabOpen(false);
      forgetRightTab(tab, rightTab === tab);
    },
    [forgetRightTab, rightTab],
  );

  // Drag the panel's left edge to resize; width persists across reloads.
  const resizePanel = (e: React.PointerEvent) => {
    e.preventDefault();
    // Capture the pointer so the terminal/diff views under the cursor don't
    // steal the drag, and suppress text selection for its duration.
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const prevUserSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    const startedMaximized = panelMax;
    const startX = e.clientX;
    const startWidth = panelWidth;
    let restoredFromMax = false;
    function stop() {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      document.body.style.userSelect = prevUserSelect;
    }
    function onMove(ev: PointerEvent) {
      if (startedMaximized) {
        const distance = ev.clientX - startX;
        if (restoredFromMax || distance < FULLSCREEN_RESTORE_DRAG) return;
        restoredFromMax = true;
        setPanelMax(false);
        const width = Math.min(
          Math.max(startWidth, PANEL_MIN_WIDTH),
          panelMaxWidth(),
        );
        setPanelWidth(width);
        window.removeEventListener("pointermove", onMove);
        return;
      }
      const w = Math.round(window.innerWidth - ev.clientX - PANEL_MARGIN);
      const max = panelMaxWidth();
      // Drag past the usable max by the slop threshold → snap to fullscreen.
      // Dragging back below it drops out of fullscreen to the clamped width.
      if (w > max + FULLSCREEN_SNAP_SLOP) {
        setPanelMax(true);
        return;
      }
      setPanelMax(false);
      const clamped = Math.min(Math.max(w, PANEL_MIN_WIDTH), max);
      setPanelWidth(clamped);
    }
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  };

  const onProjectCreated = (project: Project, publicationError: string | null) => {
    setProjects((cur) => (cur ? upsert(cur, project) : [project]));
    void router.navigate({ href: `/projects/${encodeURIComponent(project.id)}${publicationError ? "/settings/git" : ""}` });
    if (publicationError) {
      showAlert(publicationError, "error");
    }
  };

  const expTab =
    typeof rightTab === "object" && "id" in rightTab ? rightTab : null;
  const fileTab = typeof rightTab === "object" && "path" in rightTab ? rightTab : null;
  const fileArtifactEntry = fileTab?.source === "artifacts" && artifacts
    ? findArtifactEntry(artifacts.entries, fileTab.path)
    : null;
  const artifactVersion = fileArtifactEntry
    ? `${fileArtifactEntry.modifiedAt}:${fileArtifactEntry.size}`
    : null;
  const onboardingOverviewTab =
    activeSessionId === DEMO_MAIN_SESSION_ID && demoOverviewLeading
      ? fileTabs.find(
          (tab) =>
            sameFileTab(tab, {
              path: DEMO_OVERVIEW_ARTIFACT,
              source: "artifacts",
            }),
        )
      : undefined;
  // The demo overview leads the home tabs; every other content tab follows the
  // stable order in which it was opened (or reused as the preview slot).
  const leadingFileTabs = onboardingOverviewTab ? [onboardingOverviewTab] : [];
  // PlanViewDef and SubagentViewDef both carry `kind`; discriminate on its value.
  const planTab =
    typeof rightTab === "object" && "kind" in rightTab && rightTab.kind === "plan"
      ? rightTab
      : null;
  const [planContent, setPlanContent] = useState<{ key: string; text: string | null } | null>(null);
  const planSessionId = planTab?.sessionId;
  const planPromptId = planTab?.promptId;
  useEffect(() => {
    if (!planSessionId || !planPromptId || !sessions?.includes(planSessionId)) return;
    let active = true;
    const key = `${planSessionId}:${planPromptId}`;
    const apply = (messages: ChatMessage[]) => {
      const part = messages.flatMap((message) => {
        const found = findPartById(message.parts, planPromptId);
        return found ? [found] : [];
      })[0];
      if (active) setPlanContent({ key, text: part?.prompt?.plan ?? null });
    };
    const load = () => void getChatMessages(planSessionId).then(({ messages }) => apply(messages)).catch(() => { if (active) setPlanContent({ key, text: null }); });
    load();
    const off = onChatEvent((event) => {
      if (event.type === "reconnected") load();
      if (event.type === "message" && event.sessionId === planSessionId && findPartById(event.message.parts, planPromptId)) apply([event.message]);
    });
    return () => { active = false; off(); };
  }, [planSessionId, planPromptId, sessions]);
  const subagentTab =
    typeof rightTab === "object" && "kind" in rightTab && rightTab.kind === "subagent"
      ? rightTab
      : null;
  const requestedCodeTab =
    typeof rightTab === "object" && "code" in rightTab ? rightTab : null;
  const codeTab = requestedCodeTab
    ? (codeTabs.find((tab) => sameCodeTab(tab, requestedCodeTab)) ?? null)
    : null;
  const contentTabByKey = new Map<string, ContentTab>();
  for (const tab of [...expTabs, ...fileTabs, ...planTabs, ...subagentTabs, ...codeTabs]) {
    contentTabByKey.set(rightTabKey(tab), tab);
  }
  const leadingContentKey = onboardingOverviewTab
    ? rightTabKey(onboardingOverviewTab)
    : null;
  const orderedContentTabs = contentTabOrder
    .filter((key) => key !== leadingContentKey)
    .map((key) => contentTabByKey.get(key))
    .filter(isPresent);
  const isPreviewTab = (tab: RightTab) =>
    previewTab !== null && rightTabKey(previewTab) === rightTabKey(tab);
  const renderFileTab = (tab: FileViewDef) => (
    <ClosableTab
      key={`file:${fileTabKey(tab)}`}
      active={fileTab !== null && sameFileTab(fileTab, tab)}
      label={tab.path.split("/").pop() || tab.path}
      icon={<FileCode size={12} className="shrink-0" />}
      preview={isPreviewTab(tab)}
      onSelect={() => selectRightTab(tab)}
      onPromote={() => promoteRightTab(tab)}
      onClose={() => closeFileTab(tab)}
   />
  );
  const tabExperiment = expTab ? (experiments.find((e) => e.id === expTab.id) ?? null) : null;
  const codeExperiment = codeTab
    ? (experiments.find((experiment) => experiment.id === codeTab.experimentId && experiment.branchName === codeTab.branch) ?? null)
    : null;
  const renderContentTab = (tab: ContentTab) => {
    if ("path" in tab) return renderFileTab(tab);
    if ("id" in tab) {
      const experiment = experiments.find((item) => item.id === tab.id);
      return (
        <ClosableTab
          key={rightTabKey(tab)}
          active={expTab !== null && sameExpTab(expTab, tab)}
          label={experiment ? experiment.title || experiment.slug : "…"}
          icon={
            tab.view === "overview" ? (
              <ChartSpline size={12} className="shrink-0" />
            ) : (
              <Terminal size={12} className="shrink-0" />
            )
          }
          preview={isPreviewTab(tab)}
          onSelect={() => selectRightTab(tab)}
          onPromote={() => promoteRightTab(tab)}
          onClose={() => closeExperimentTab(tab)}
       />
      );
    }
    if ("kind" in tab && tab.kind === "plan") {
      return (
        <ClosableTab
          key={rightTabKey(tab)}
          active={planTab !== null && planTab.promptId === tab.promptId}
          label={m.chat_plan()}
          icon={<ScrollText size={12} className="shrink-0" />}
          preview={isPreviewTab(tab)}
          onSelect={() => selectRightTab(tab)}
          onPromote={() => promoteRightTab(tab)}
          onClose={() => closePlanTab(tab)}
       />
      );
    }
    if ("kind" in tab) {
      return (
        <ClosableTab
          key={rightTabKey(tab)}
          active={subagentTab !== null && subagentTab.spawnPartId === tab.spawnPartId}
          label={spawnMeta[tab.spawnPartId]?.label ?? tab.label ?? m.app_subagent()}
          shimmer={spawnMeta[tab.spawnPartId]?.running ?? false}
          icon={<Users size={12} className="shrink-0" />}
          preview={isPreviewTab(tab)}
          onSelect={() => selectRightTab(tab)}
          onPromote={() => promoteRightTab(tab)}
          onClose={() => closeSubagentTab(tab)}
       />
      );
    }
    const experiment = experiments.find((item) => item.id === tab.experimentId);
    return (
      <ClosableTab
        key={rightTabKey(tab)}
        active={codeTab !== null && sameCodeTab(codeTab, tab)}
        label={experiment?.slug ?? tab.branch}
        icon={<FolderOpen size={12} className="shrink-0" />}
        preview={isPreviewTab(tab)}
        onSelect={() => selectRightTab(tab)}
        onPromote={() => promoteRightTab(tab)}
        onClose={() => closeCodeTab(tab)}
     />
    );
  };

  if (!destination || destination.kind === "resume") return null;

  if (startupError) {
    return (
      <div className="app flex flex-col h-full">
        <div className={EMPTY_STATE_CLASS_NAME}>
          <p>{startupError}</p>
          <Button variant="primary" onClick={loadInitialState}>{m.app_retry()}</Button>
        </div>
        {runtime.kind === "ssh" && <RemoteStatus runtime={runtime} corner />}
      </div>
    );
  }

  if (projects && !activeProject) {
    return <div className={EMPTY_STATE_CLASS_NAME}>{m.model_picker_unavailable()}<Button onClick={() => setProjectId(null)}>{m.app_projects()}</Button></div>;
  }

  if (workspaceError && !workspaceReady) {
    return <div className={EMPTY_STATE_CLASS_NAME}><p role="alert">{workspaceError}</p><Button onClick={retryWorkspace}>{m.app_retry()}</Button></div>;
  }

  if (projects === null || uiState === null || !workspaceLoaded || sessions === null) {
    return (
      <div className="app flex flex-col h-full">
        <div className={EMPTY_STATE_CLASS_NAME}>
          <Spinner />
        </div>
        {runtime.kind === "ssh" && <RemoteStatus runtime={runtime} corner />}
      </div>
    );
  }

  if (!activeProject || (destination?.kind === "task" && activeSessionId && !sessions?.includes(activeSessionId))) {
    return <div className={EMPTY_STATE_CLASS_NAME}>{m.model_picker_unavailable()}<Button onClick={() => setProjectId(null)}>{m.app_projects()}</Button></div>;
  }

  const railHeader = (
    <RailHeader
      projectName={projects.find((p) => p.id === projectId)?.name ?? ""}
      onHome={() => void router.navigate({ to: "/projects" })}
      onNewProject={() => setNewProjectOpen(true)}
      onRepository={() => selectMainView("git")}
      onCollapse={() => setRailOpen(false)}
   />
  );

  return (
    <div className="app flex flex-col h-full">
      {runtime.kind === "local" && <OfflineBanner />}
      {runtime.kind === "local" && <UpdateBanner status={updateStatus} />}
      {workspaceError && <div role="alert" className="flex items-center gap-2 px-4 py-2 text-subtext"><span>{workspaceError}</span><Button onClick={retryWorkspace}>{m.app_retry()}</Button></div>}
      <div className="app-body flex flex-1 min-h-0 py-0 px-3.5">
        {projectId && (
          <ChatPanel
            projectId={projectId}
            projectName={activeProject?.name ?? ""}
            railHeader={railHeader}
            railOpen={railOpen}
            onShowRail={() => setRailOpen(true)}
            mainView={mainView}
            onSelectMainView={selectMainView}
            experimentsActive={
              mainView === "chat" && panelOpen && rightTab === "experiments"
            }
            filesActive={mainView === "chat" && panelOpen && rightTab === "files"}
            artifactsActive={mainView === "chat" && panelOpen && rightTab === "artifacts"}
            onOpenExperiments={() => openExperimentsTab()}
            onOpenArtifacts={openArtifactsTab}
            onOpenFile={openChatFile}
            onOpenRun={openRunLogs}
            runExperimentName={runExperimentName}
            onOpenExperiment={openExperimentNotes}
            experimentName={experimentName}
            onOpenPlan={openPlanTab}
            onOpenSubagent={openSubagentTab}
            onOpenWorktree={openWorktreeTab}
            composerPrefill={
              activeProject &&
              isDemoProjectId(activeProject.id) &&
              uiState?.tourCompleted === false
                ? DEMO_RUN_EXPERIMENT_PROMPT
                : null
            }
            runtime={runtime}
            onOpenDemoWelcome={
              activeProject && isDemoProjectId(activeProject.id) ? openDemoWelcome : undefined
            }
            activeSessionId={activeSessionId}
            onActiveSessionChange={onActiveSessionChange}
            preferredAgent={uiState.preferredAgent}
            onPreferredAgentChange={persistPreferredAgent}
          >
            {mainView === "skills" ? (
              <SkillsTab />
            ) : mainView !== "chat" ? (
              <SettingsView
                remote={runtime.kind === "ssh"}
                tab={mainView}
                project={activeProject}
                onProjectUpdate={(project) => {
                  setProjects((current) => (current ? upsert(current, project) : [project]));
                }}
                onSelectTab={selectMainView}
             />
            ) : null}
          </ChatPanel>
        )}
        {mainView === "chat" && panelOpen && (
        <aside
          className={`right-pane relative shrink-0 min-w-0 flex flex-col mt-5 me-0 mb-5 ms-3.5 bg-canvas [&.max]:fixed [&.max]:inset-2.5 [&.max]:m-0 [&.max]:z-60 [&.max]:shadow-panel-max border border-border rounded-lg overflow-hidden shadow-elevated ${panelMax ? "max" : ""}`}
          style={panelMax ? undefined : { width: panelWidth }}
          data-onboarding="experiments"
        >
          <div
            className={`panel-resizer absolute start-0 top-0 bottom-0 w-1.5 z-30 [&:hover]:bg-resizer-hover [&:active]:bg-resizer-hover ${panelMax ? "cursor-e-resize" : "cursor-col-resize"}`}
            title={panelMax ? m.app_drag_to_restore_panel() : m.app_drag_to_resize_panel()}
            onPointerDown={resizePanel}
         />
          <div className="tabs flex items-end gap-0 pt-1 pe-1.5 pb-0 ps-2 h-10 border-b border-b-border bg-background shrink-0">
            <div className="tab-strip flex items-end gap-0.5 flex-1 min-w-0 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              {leadingFileTabs.map(renderFileTab)}
              {filesTabOpen && (
                <ClosableTab
                  active={rightTab === "files"}
                  label={m.app_files()}
                  icon={<FolderOpen size={12} className="shrink-0" />}
                  onSelect={() => selectRightTab("files")}
                  onClose={() => closeHomeTab("files")}
               />
              )}
              {artifactsTabOpen && (
                <ClosableTab
                  active={rightTab === "artifacts"}
                  label={m.app_artifacts()}
                  icon={<Package size={12} className="shrink-0" />}
                  onSelect={() => selectRightTab("artifacts")}
                  onClose={() => closeHomeTab("artifacts")}
               />
              )}
              {experimentsTabOpen && (
                <ClosableTab
                  active={rightTab === "experiments"}
                  label={m.app_experiments()}
                  icon={<FlaskConical size={12} className="shrink-0" />}
                  onSelect={() => selectRightTab("experiments")}
                  onClose={() => closeHomeTab("experiments")}
               />
              )}
              {orderedContentTabs.map(renderContentTab)}
            </div>
            <div className="panel-controls inline-flex items-center gap-0.5 self-center py-0 px-1.5 shrink-0">
              <IconButton
                title={panelMax ? m.app_restore_panel() : m.app_expand_panel()}
                aria-label={panelMax ? m.app_restore_panel() : m.app_expand_panel()}
                onClick={() => setPanelMax((m) => !m)}
              >
                {panelMax ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
              </IconButton>
              <IconButton
                title={m.app_close_panel()}
                aria-label={m.app_close_panel()}
                onClick={() => {
                  closePanel();
                  setPanelMax(false);
                }}
              >
                <X size={14} />
              </IconButton>
            </div>
          </div>
          {!workspaceReady || ((pane?.kind === "experiment" || pane?.kind === "code") && !experimentDataReady) || (pane?.kind === "experiment" && pane.runId && !runDataReady) ? (
            <TabBody><Spinner /></TabBody>
          ) : (expTab && (!tabExperiment || (selectedRunId && !runs.some((run) => run.id === selectedRunId && run.experimentId === expTab.id))))
            || (requestedCodeTab && !codeExperiment)
            || (pane && "sessionId" in pane && pane.sessionId && !sessions?.includes(pane.sessionId)) ? (
            <TabBody><div className="p-6 text-subtext">{m.model_picker_unavailable()}</div></TabBody>
          ) : rightTab === "artifacts" ? (
            <TabBody>
              {activeProject && (
                <ArtifactsTab
                  key={activeProject.id}
                  project={activeProject}
                  artifacts={artifacts}
                  onChanged={refreshArtifacts}
                  onOpenFile={openArtifactFileTab}
                  canRenameFile={(path) => !fileBuffersRef.current.get(
                    fileScrollKey(activeProject.id, activeSessionId, { path, source: "artifacts" }),
                  )?.needsProtection}
                  onOpenStorage={runtime.kind === "ssh" ? undefined : () => selectMainView("storage")}
               />
              )}
            </TabBody>
          ) : rightTab === "experiments" ? (
            <TabBody>
              <div className="pane-toolbar flex shrink-0 flex-wrap items-center gap-2 bg-background px-3 pt-2.5 pb-2">
                <span className="flex-1" />
                <div className="experiments-toolbar-controls inline-flex items-center gap-[5px]">
                  <div className="option-picker relative inline-flex" ref={scopeMenuRef}>
                    <IconButton size="small"
                      ref={scopeTriggerRef}
                      className="experiment-scope-trigger"
                      active={effectiveScope === "agent"}
                      title={m.app_experiment_filter({ scope: effectiveScope === "agent" ? m.app_current_task() : m.app_entire_project() })}
                      aria-label={m.app_filter_experiments()}
                      aria-expanded={scopeMenuOpen}
                      onClick={() => setScopeMenuOpen((open) => !open)}
                    >
                      <Filter size={16} strokeWidth={2.5} />
                    </IconButton>
                    {scopeMenuOpen && (
                      <div className="option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down align-right experiment-scope-menu [&_.model-item]:whitespace-nowrap [&_.model-item:disabled]:text-muted [&_.model-item:disabled]:cursor-default [&_.model-item:disabled:hover]:bg-transparent">
                        <MenuItem
                          aria-pressed={effectiveScope === "agent"}
                          disabled={!activeSessionId || !allExperimentsAttributed}
                          title={
                            !activeSessionId
                              ? m.app_open_task_to_filter()
                              : !allExperimentsAttributed
                                ? m.app_filter_unavailable_unattributed()
                                : undefined
                          }
                          onClick={() => {
                            setScope("agent");
                            setScopeMenuOpen(false);
                          }}
                        >
                          <span>{m.app_current_task()}</span>
                          {effectiveScope === "agent" && <Check size={13} />}
                        </MenuItem>
                        <MenuItem
                          aria-pressed={effectiveScope === "project"}
                          onClick={() => {
                            setScope("project");
                            setScopeMenuOpen(false);
                          }}
                        >
                          <span>{m.app_entire_project()}</span>
                          {effectiveScope === "project" && <Check size={13} />}
                        </MenuItem>
                      </div>
                    )}
                  </div>
                  <div
                    className="seg inline-flex items-center gap-0.5 rounded-md bg-hover-subtle [&_button]:font-medium [&_button]:text-text [&_button]:rounded-sm [&_button:not(:disabled):hover]:text-text [&_button.active]:bg-background [&_button.active]:shadow-segment [&_button:disabled]:text-muted [&_button:disabled]:cursor-default experiments-view-toggle p-0.5 [&_button]:py-0.5 [&_button]:px-2 [&_button]:text-sm"
                    role="group"
                    aria-label={m.app_experiment_view()}
                  >
                    <button
                      className={view === "table" ? "active" : ""}
                      aria-pressed={view === "table"}
                      onClick={() => setView("table")}
                    >
                      {m.app_table()}
                    </button>
                    <button
                      className={view === "tree" ? "active" : ""}
                      aria-pressed={view === "tree"}
                      onClick={() => setView("tree")}
                    >
                      {m.app_tree()}
                    </button>
                  </div>
                </div>
              </div>
              <div className="pane-content flex-1 min-h-0 relative bg-background">
                {view === "tree" ? (
                  activeProject && (
                    <TreeView
                      experiments={experiments}
                      runs={scopedRuns}
                      project={activeProject}
                      onOpenView={openExperimentTab}
                      onOpenCode={openCodeTabForExperiment}
                      agentSessionId={effectiveScope === "agent" ? activeSessionId : null}
                      onShowProjectScope={showProjectScope}
                   />
                  )
                ) : (
                  <ExperimentsTable
                    runs={scopedRuns}
                    emptyHint={
                      effectiveScope === "agent" && experiments.length > 0
                        ? m.app_no_task_experiments()
                        : undefined
                    }
                    experiments={scopedExperiments}
                    onOpen={(experiment, intent) => {
                      openExperimentTab(experiment.id, "overview", intent);
                    }}
                    onOpenLogs={(experimentId, runId, intent) => {
                      openExperimentTab(experimentId, "terminal", intent, runId);
                    }}
                    onOpenCode={(experimentId, intent) => {
                      const experiment = experiments.find((item) => item.id === experimentId);
                      if (experiment)
                        openCodeTabForExperiment(
                          experiment.id,
                          experiment.branchName,
                          "files",
                          intent,
                        );
                    }}
                    onCancel={cancelRun}
                 />
                )}
              </div>
            </TabBody>
          ) : rightTab === "files" ? (
            <TabBody>
              {activeProject ? (
                <WorktreeTab
                  key={`files:${activeSessionId ?? `project:${activeProject.id}`}`}
                  sessionId={activeSessionId ?? undefined}
                  project={activeProject}
                  view={filesView}
                  toggled={filesToggled}
                  onViewChange={setFilesView}
                  onToggledChange={setFilesToggled}
                  canRenameFile={(path) => !fileBuffersRef.current.get(
                    fileScrollKey(activeProject.id, activeSessionId, {
                      path,
                      source: "repo",
                      sessionId: activeSessionId ?? undefined,
                    }),
                  )?.needsProtection}
                  onOpenFile={(path, sessionId, ref, intent) =>
                    openFileTab(
                      path,
                      sessionId,
                      ref,
                      undefined,
                      undefined,
                      undefined,
                      intent,
                    )
                  }
               />
              ) : (
                <div className="code-tab flex flex-col h-full min-h-0 wt-tab">
                  <CodeTabBody>
                    <div className="wt-empty flex flex-col items-center gap-2.5 py-12 px-6 text-center text-muted [&_>_svg]:text-subtext [&_p]:m-0 [&_p]:max-w-80 [&_p]:text-sm">
                      <FolderGit2 size={22} />
                      <p>{m.app_select_a_project_to_browse_its_files()}</p>
                    </div>
                  </CodeTabBody>
                </div>
              )}
            </TabBody>
          ) : fileTab ? (
            <TabBody>
              {projectId && (
                <FileViewer
                  remote={runtime.kind === "ssh"}
                  restored={restoredFilesRef.current.has(fileScrollKey(projectId, activeSessionId, fileTab))}
                  onRestoreActivated={() => {
                    const key = fileScrollKey(projectId, activeSessionId, fileTab);
                    restoredFilesRef.current.delete(key);
                    intentionalFilesRef.current.add(key);
                  }}
                  showSource={sourceModesRef.current[fileScrollKey(projectId, activeSessionId, fileTab)] ?? false}
                  onShowSourceChange={(showSource) => {
                    sourceModesRef.current[fileScrollKey(projectId, activeSessionId, fileTab)] = showSource;
                    setMetadataRevision((value) => value + 1);
                  }}
                  key={fileScrollKey(projectId, activeSessionId, fileTab)}
                  projectId={projectId}
                  path={fileTab.path}
                  source={fileTab.source}
                  // Artifacts tabs fall back to the checkout, which needs this session.
                  sessionId={
                    fileTab.source === "artifacts"
                      ? (activeSessionId ?? undefined)
                      : fileTab.sessionId
                  }
                  gitRef={fileTab.ref}
                  line={fileTab.line}
                  branchLabel={fileBranchLabel(fileTab, activeProject?.baselineBranch)}
                  artifactVersion={artifactVersion}
                  artifactEntries={fileTab.source === "artifacts" ? artifacts?.entries : undefined}
                  bufferSession={getFileBufferSession(fileScrollKey(projectId, activeSessionId, fileTab))}
                  onOpenFile={(path, sessionId, ref, intent) =>
                    openFromRightTab(fileTab, () =>
                      openFileTab(
                        path,
                        sessionId,
                        ref,
                        undefined,
                        undefined,
                        undefined,
                        intent,
                      ),
                    )
                  }
                  scrollPosition={fileScrollPositionsRef.current.get(
                    fileScrollKey(projectId, activeSessionId, fileTab),
                  )}
                  onScrollPositionChange={(position) => {
                    fileScrollPositionsRef.current.set(
                      fileScrollKey(projectId, activeSessionId, fileTab),
                      position,
                    );
                    recordScroll();
                  }}
                  lineScrollRequest={fileTab.lineScrollRequest}
                  onLineScrollRequestHandled={() => consumeFileLineScrollRequest(fileTab)}
                  onEdit={() => promoteRightTab(fileTab)}
               />
              )}
            </TabBody>
          ) : planTab ? (
            <TabBody>
              {/* The plan markdown is already client-side — render directly,
                  file links resolve against the plan's session worktree. */}
              <div className="pane-content flex-1 min-h-0 relative plan-tab-content overflow-y-auto bg-background py-4.5 px-6 [&_.md]:max-w-readable">
                <Md
                  text={planContent?.key === `${planTab.sessionId}:${planTab.promptId}` ? planContent.text ?? m.model_picker_unavailable() : planTabs.find((tab) => tab.promptId === planTab.promptId)?.plan || m.artifacts_tab_loading()}
                  onOpenFile={(path, line, exp, ref, intent) =>
                    openFromRightTab(planTab, () =>
                      openFileTab(
                        path,
                        planTab.sessionId,
                        ref,
                        line,
                        exp,
                        undefined,
                        intent,
                      ),
                    )
                  }
               />
              </div>
            </TabBody>
          ) : subagentTab ? (
            <SubagentTab
              // Remount per spawn part so the seed + subscription reset cleanly.
              key={subagentTab.spawnPartId}
              sessionId={subagentTab.sessionId}
              spawnPartId={subagentTab.spawnPartId}
              onOpenFile={(path, line, exp, ref, intent) =>
                openFromRightTab(subagentTab, () =>
                  openChatFile(path, subagentTab.sessionId, line, exp, ref, intent),
                )
              }
              onOpenRun={(runId, intent) =>
                openFromRightTab(subagentTab, () => openRunLogs(runId, intent))
              }
              runExperimentName={runExperimentName}
              onOpenExperiment={(experimentId, intent) =>
                openFromRightTab(subagentTab, () => openExperimentNotes(experimentId, intent))
              }
              experimentName={experimentName}
              onOpenSubagent={(pid, label, intent) =>
                openFromRightTab(subagentTab, () =>
                  openSubagentTab(subagentTab.sessionId, pid, label, intent),
                )
              }
           />
          ) : codeTab ? (
            <TabBody>
              {projectId && activeProject && codeTab && codeExperiment && (
                <CodeTab
                  key={`code:${codeTab.branch}`}
                  projectId={projectId}
                  project={activeProject}
                  experiment={codeExperiment}
                  view={codeTab.view}
                  toggled={codeTab.toggled}
                  onViewChange={(view) => updateCodeTab(codeTab, { view })}
                  onToggledChange={(toggled) => updateCodeTab(codeTab, { toggled })}
                  onOpenFile={(path, sessionId, ref, intent) =>
                    openFromRightTab(codeTab, () =>
                      openFileTab(
                        path,
                        sessionId,
                        ref,
                        undefined,
                        undefined,
                        codeExperiment.branchName,
                        intent,
                      ),
                    )
                  }
               />
              )}
            </TabBody>
          ) : (
            <TabBody>
              {expTab && tabExperiment && activeProject && (
                <DetailDrawer
                  key={`${expTab.id}:${expTab.view}`}
                  experiment={tabExperiment}
                  project={activeProject}
                  view={expTab.view}
                  runs={runs}
                  selectedRunId={selectedRunId}
                  onSelectRun={setSelectedRunId}
                  parentExperiment={
                    experiments.find(
                      (experiment) => experiment.id === tabExperiment.parentExperimentId,
                    ) ?? null
                  }
                  onOpenView={(view, runId, intent) => {
                    openFromRightTab(expTab, () =>
                      openExperimentTab(tabExperiment.id, view, intent, runId),
                    );
                  }}
                  onOpenCode={(view, intent) =>
                    openFromRightTab(expTab, () =>
                      openCodeTabForExperiment(
                        tabExperiment.id,
                        tabExperiment.branchName,
                        view,
                        intent,
                      ),
                    )
                  }
               />
              )}
            </TabBody>
          )}
        </aside>
        )}
      </div>
      {newProjectOpen && (
        <NewProjectDialog
          remote={runtime.kind === "ssh"}
          onClose={() => setNewProjectOpen(false)}
          onCreated={(project, publicationError) => {
            setNewProjectOpen(false);
            onProjectCreated(project, publicationError);
          }}
       />
      )}
      {demoWelcomeOpen && activeProject && isDemoProjectId(activeProject.id) && (
        <DemoWelcomeModal
          onClose={closeDemoWelcome}
          onCreateProject={createProjectFromDemoWelcome}
       />
      )}
    </div>
  );
}
