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
  DEMO_FIGURE_SESSION_ID,
  DEMO_LITERATURE_SESSION_ID,
  getArtifacts,
  getUiState,
  listExperiments,
  listProjects,
  listRuns,
  openProject,
  updateUiState,
  type AgentSelection,
  type Experiment,
  type ProjectArtifacts,
  type Project,
  type Run,
  type UiState,
} from "./api";
import { ChatPanel } from "./components/ChatPanel";
import { SubagentTab } from "./components/SubagentTab";
import { CodeTab, type CodeView } from "./components/CodeTab";
import { WorktreeTab, type WorktreeView } from "./components/WorktreeTab";
import { ArtifactsTab } from "./components/ArtifactsTab";
import { ClosableTab } from "./components/ClosableTab";
import { DetailDrawer, type ExperimentView } from "./components/DetailDrawer";
import { FileViewer } from "./components/FileViewer";
import { RailHeader } from "./components/Header";
import { Onboarding } from "./components/Onboarding";
import { NewProjectDialog, ProjectsHome } from "./components/ProjectsHome";
import { ExperimentsTable } from "./components/ExperimentsTable";
import { Md } from "./components/Md";
import { usePopover } from "./components/ModelPicker";
import { SettingsView, type SettingsTab } from "./components/SettingsPage";
import { Tour } from "./components/Tour";
import { clearReadDemoSessions } from "./demoSessionState";
import { TreeView } from "./components/TreeView";
import { useOrxEvents } from "./events";
import { CODE_TAB_BODY_CLASS_NAME, ICON_BUTTON_BASE_CLASS_NAME, ICON_BUTTON_CLASS_NAME, MODEL_ITEM_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME, TAB_BODY_CLASS_NAME } from "./styleClasses";

const EMPTY_STATE_CLASS_NAME = [
  "empty-state absolute inset-0 flex flex-col items-center",
  "justify-center gap-2.5 p-6 text-center text-subtext",
  "[&_p]:max-w-[46ch] [&_p]:m-0 [&_p]:text-md [&_p]:leading-normal",
  "[&_p]:text-balance [&_p.empty-state-title]:text-2xl",
  "[&_p.empty-state-title]:font-normal [&_p.empty-state-title]:text-text",
  "[&_p.empty-state-hint]:text-lg [&_p.empty-state-hint]:text-subtext",
].join(" ");

/** An experiment view open as a right-panel tab. */
interface ExpViewDef {
  id: string;
  view: ExperimentView;
}

const sameExpTab = (a: ExpViewDef, b: ExpViewDef) => a.id === b.id && a.view === b.view;

/** A project file open as a right-panel tab (clicked in chat tool rows or the
 * code browser). */
interface FileViewDef {
  path: string;
  /** Which backend serves this file. Absent/"repo" → the repo `/file`
   * endpoint (worktree/clone/branch), falling back to artifacts when a
   * non-ref path misses the checkout; "artifacts" → the project's durable
   * output directory through the compatibility `/files/file` endpoint. */
  source?: "repo" | "artifacts";
  /** Chat session whose worktree holds the file (absent → hub clone).
   * Artifact tabs never carry this. */
  sessionId?: string;
  /** Branch whose committed copy to show (code browser in branch mode);
   * overrides the live checkout. */
  ref?: string;
  /** 1-based line to scroll to and highlight on open (from a `file:line`
   * evidence chip). Not part of tab identity — reopening at a new line updates
   * the same tab. */
  line?: number;
}

const sameFileTab = (a: FileViewDef, b: FileViewDef) =>
  a.path === b.path &&
  (a.source ?? "repo") === (b.source ?? "repo") &&
  a.sessionId === b.sessionId &&
  a.ref === b.ref;

const fileTabKey = (t: FileViewDef) =>
  `${t.source ?? "repo"}:${t.sessionId ?? ""}:${t.ref ?? ""}:${t.path}`;

/** A proposed plan open as a right-panel tab (from the chat plan strip/card).
 * The markdown is already client-side (it rode the prompt part), so the tab
 * renders it directly — no fetch. Deliberately has neither a `view` nor a
 * `path` field: the other tab kinds discriminate on those. */
interface PlanViewDef {
  kind: "plan";
  sessionId: string;
  /** The prompt part the plan came from — one tab per plan card. */
  promptId: string;
  plan: string;
}

/** A sub-agent's transcript, opened from a chat spawn row's "view" button. One
 * tab per spawn part; its parts stream live off the session's chat message. */
interface SubagentViewDef {
  kind: "subagent";
  sessionId: string;
  /** The `subagent` spawn part whose `children` are the sub-agent transcript. */
  spawnPartId: string;
}

/** One committed code-browser tab per experiment branch. Source, selected
 * view, and expansion state live here so they survive tab switches. */
interface CodeTabDef {
  code: true;
  experimentId: string;
  branch: string;
  view: CodeView;
  /** Dirs the user flipped away from their depth default. */
  toggled: ReadonlySet<string>;
}

const sameCodeTab = (a: CodeTabDef, b: CodeTabDef) => a.branch === b.branch;

type RightTab =
  | "experiments"
  | "files"
  | "artifacts"
  | ExpViewDef
  | FileViewDef
  | PlanViewDef
  | SubagentViewDef
  | CodeTabDef;

function rightTabKey(tab: RightTab): string {
  if (typeof tab === "string") return `home:${tab}`;
  if ("code" in tab) return `code:${tab.branch}`;
  if ("kind" in tab) {
    return tab.kind === "plan" ? `plan:${tab.promptId}` : `subagent:${tab.spawnPartId}`;
  }
  if ("path" in tab) return `file:${fileTabKey(tab)}`;
  return `experiment:${tab.id}:${tab.view}`;
}

interface RightPaneSessionState {
  rightTab: RightTab;
  tabHistory: RightTab[];
  experimentsTabOpen: boolean;
  filesTabOpen: boolean;
  artifactsTabOpen: boolean;
  expTabs: ExpViewDef[];
  fileTabs: FileViewDef[];
  planTabs: PlanViewDef[];
  subagentTabs: SubagentViewDef[];
  codeTabs: CodeTabDef[];
  filesView: WorktreeView;
  filesToggled: ReadonlySet<string>;
  selectedRunId: string | null;
  scope: "agent" | "project";
  panelOpen: boolean;
  panelMax: boolean;
}

function initialRightPaneSessionState(sessionId?: string): RightPaneSessionState {
  const initial: RightPaneSessionState = {
    rightTab: "experiments",
    tabHistory: ["experiments"],
    experimentsTabOpen: true,
    filesTabOpen: false,
    artifactsTabOpen: false,
    expTabs: [],
    fileTabs: [],
    planTabs: [],
    subagentTabs: [],
    codeTabs: [],
    filesView: "files",
    filesToggled: new Set(),
    selectedRunId: null,
    scope: "project",
    panelOpen: true,
    panelMax: false,
  };
  if (sessionId === DEMO_FIGURE_SESSION_ID) {
    const fileTabs: FileViewDef[] = [
      { path: "nanochat-base-training-curves.svg", source: "artifacts" },
      { path: "nanochat-sft-training-curves.svg", source: "artifacts" },
      { path: "nanochat-training-throughput.svg", source: "artifacts" },
      { path: "nanochat-core-evaluation.svg", source: "artifacts" },
    ];
    return {
      ...initial,
      rightTab: fileTabs[0],
      tabHistory: [...fileTabs.slice(1), fileTabs[0]],
      experimentsTabOpen: false,
      fileTabs,
    };
  }
  if (sessionId === DEMO_LITERATURE_SESSION_ID) {
    const fileTabs: FileViewDef[] = [
      { path: "nanochat-bottleneck-diagnosis.md", source: "artifacts" },
    ];
    return {
      ...initial,
      rightTab: fileTabs[0],
      tabHistory: [fileTabs[0]],
      experimentsTabOpen: false,
      fileTabs,
    };
  }
  return initial;
}

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
  if (!path.startsWith("/")) {
    sessionId = contextSessionId;
  } else if (artifacts && (path === artifacts || path.startsWith(`${artifacts}/`))) {
    // Artifact — exact prefix match against the non-canonical dir the
    // backend surfaced, which mirrors what the agent inlines.
    const rel = path.slice(artifacts.length).replace(/^\/+/, "");
    return rel ? { path: rel, source: "artifacts" } : null;
  } else if (clone && (path === clone || path.startsWith(`${clone}/`))) {
    path = path.slice(clone.length).replace(/^\/+/, "");
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
  return path ? { path, sessionId } : null;
}

/** The git branch a code file tab is showing, for the header pill — a cited
 * experiment's branch (or any ref view) names that branch, and a worktree/clone
 * file falls back to the baseline branch, so a code tab always says which
 * branch its contents came from. Artifacts have no branch. */
function fileBranchLabel(tab: FileViewDef, baselineBranch?: string): string | undefined {
  if (tab.source === "artifacts") return undefined;
  return tab.ref ?? baselineBranch;
}

const PANEL_WIDTH_KEY = "orx:panel-width";
const EXPERIMENTS_VIEW_KEY = "orx:experiments-view";

type ExperimentsView = "tree" | "table";

function initialExperimentsView(): ExperimentsView {
  try {
    return localStorage.getItem(EXPERIMENTS_VIEW_KEY) === "tree" ? "tree" : "table";
  } catch {
    return "table";
  }
}

/** Floating panel sizing: keep both the panel and the chat column usable. */
const PANEL_MIN_WIDTH = 360;
const PANEL_MARGIN = 10;
// Space the rest of the layout needs beside the panel: the 272px rail, the
// chat column's minimum, and the gutters/margins between the three columns
// (app-body padding 14×2, rail inner margin 14, right-pane inner margin 14).
const RAIL_WIDTH = 272;
const CHAT_MIN_SPACE = 380;
const LAYOUT_CHROME = RAIL_WIDTH + 14 * 4;
// Once a drag pushes the panel past its usable max by this much, it snaps to
// fullscreen — a bit of resistance you have to overcome deliberately.
const FULLSCREEN_SNAP_SLOP = 80;

/** The widest the floating panel can be while leaving the rail + chat usable. */
function panelMaxWidth(): number {
  return Math.max(PANEL_MIN_WIDTH, window.innerWidth - LAYOUT_CHROME - CHAT_MIN_SPACE);
}

function initialPanelWidth(): number {
  const max = panelMaxWidth();
  try {
    const saved = Number(localStorage.getItem(PANEL_WIDTH_KEY));
    if (Number.isFinite(saved) && saved >= PANEL_MIN_WIDTH) return Math.min(saved, max);
  } catch {
    // storage unavailable — fall through to the default
  }
  return Math.max(PANEL_MIN_WIDTH, Math.min(760, max, Math.round(window.innerWidth * 0.42)));
}

function upsert<T extends { id: string }>(list: T[], item: T): T[] {
  const i = list.findIndex((x) => x.id === item.id);
  if (i < 0) return [...list, item];
  const next = list.slice();
  next[i] = item;
  return next;
}

export default function App() {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [uiState, setUiState] = useState<UiState | null>(null);
  const [startupError, setStartupError] = useState<string | null>(null);
  const persistedPreferredAgent = useRef<AgentSelection | null>(null);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [experiments, setExperiments] = useState<Experiment[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  // Latest runs/experiments, read by the stable openRunLogs/openFileTab so
  // evidence chips resolve ids without re-creating the callbacks on every poll
  // (they feed the memoized transcript, which needs stable props).
  const runsRef = useRef(runs);
  runsRef.current = runs;
  const experimentsRef = useRef(experiments);
  experimentsRef.current = experiments;
  const [artifacts, setArtifacts] = useState<ProjectArtifacts | null>(null);
  const [view, setView] = useState<ExperimentsView>(initialExperimentsView);
  // Experiments pane scope: "agent" narrows to the open chat session's work.
  // Falls back to "project" whenever there is no usable experiment attribution.
  const [scope, setScope] = useState<"agent" | "project">("project");
  const scopeTriggerRef = useRef<HTMLButtonElement>(null);
  const { open: scopeMenuOpen, setOpen: setScopeMenuOpen, ref: scopeMenuRef } =
    usePopover(scopeTriggerRef);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
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
  useEffect(() => {
    try {
      localStorage.setItem(EXPERIMENTS_VIEW_KEY, view);
    } catch {
      // The preference remains sticky for this app session when storage is unavailable.
    }
  }, [view]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  // Right-panel tab strip: closable home and working tabs. The same experiment
  // can keep both its overview and terminal open.
  const [rightTab, setRightTab] = useState<RightTab>("experiments");
  const [tabHistory, setTabHistory] = useState<RightTab[]>(["experiments"]);
  const [experimentsTabOpen, setExperimentsTabOpen] = useState(true);
  const [filesTabOpen, setFilesTabOpen] = useState(false);
  const [artifactsTabOpen, setArtifactsTabOpen] = useState(false);
  const [expTabs, setExpTabs] = useState<ExpViewDef[]>([]);
  const [fileTabs, setFileTabs] = useState<FileViewDef[]>([]);
  const [planTabs, setPlanTabs] = useState<PlanViewDef[]>([]);
  const [subagentTabs, setSubagentTabs] = useState<SubagentViewDef[]>([]);
  const [codeTabs, setCodeTabs] = useState<CodeTabDef[]>([]);
  const [filesView, setFilesView] = useState<WorktreeView>("files");
  const [filesToggled, setFilesToggled] = useState<ReadonlySet<string>>(new Set());
  // The right pane is a floating panel: closable, edge-resizable, expandable
  // to (nearly) full screen. Width persists across sessions.
  const [panelOpen, setPanelOpen] = useState(true);
  const [panelMax, setPanelMax] = useState(false);
  const [panelWidth, setPanelWidth] = useState(initialPanelWidth);
  // The agents rail is a floating panel too: fixed-width, collapsible.
  const [railOpen, setRailOpen] = useState(true);
  const [homeOpen, setHomeOpen] = useState(false);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [mainView, setMainView] = useState<"chat" | SettingsTab>("chat");
  const [githubPublicationError, setGithubPublicationError] = useState<{
    projectId: string;
    message: string;
  } | null>(null);
  const rightPaneStatesRef = useRef(new Map<string, RightPaneSessionState>());
  const currentRightPaneStateRef = useRef<RightPaneSessionState>(initialRightPaneSessionState());
  const activeSessionIdRef = useRef<string | null>(null);
  const tabHistoryRef = useRef(tabHistory);
  tabHistoryRef.current = tabHistory;

  const selectRightTab = useCallback((tab: RightTab) => {
    const key = rightTabKey(tab);
    const next = [...tabHistoryRef.current.filter((item) => rightTabKey(item) !== key), tab];
    tabHistoryRef.current = next;
    setTabHistory(next);
    setRightTab(tab);
  }, []);

  const forgetRightTab = useCallback((tab: RightTab, selectFallback: boolean) => {
    const key = rightTabKey(tab);
    const next = tabHistoryRef.current.filter((item) => rightTabKey(item) !== key);
    tabHistoryRef.current = next;
    setTabHistory(next);
    if (!selectFallback) return;
    const fallback = next[next.length - 1];
    if (fallback) setRightTab(fallback);
    else {
      setPanelOpen(false);
      setPanelMax(false);
    }
  }, []);

  currentRightPaneStateRef.current = {
    rightTab,
    tabHistory,
    experimentsTabOpen,
    filesTabOpen,
    artifactsTabOpen,
    expTabs,
    fileTabs,
    planTabs,
    subagentTabs,
    codeTabs,
    filesView,
    filesToggled,
    selectedRunId,
    scope,
    panelOpen,
    panelMax,
  };
  const onActiveSessionChange = useCallback((nextSessionId: string | null) => {
    const previousSessionId = activeSessionIdRef.current;
    if (previousSessionId === nextSessionId) return;
    if (previousSessionId) {
      rightPaneStatesRef.current.set(previousSessionId, currentRightPaneStateRef.current);
    }
    const nextState = nextSessionId
      ? (rightPaneStatesRef.current.get(nextSessionId) ??
        initialRightPaneSessionState(nextSessionId))
      : initialRightPaneSessionState();
    setRightTab(nextState.rightTab);
    tabHistoryRef.current = nextState.tabHistory;
    setTabHistory(nextState.tabHistory);
    setExperimentsTabOpen(nextState.experimentsTabOpen);
    setFilesTabOpen(nextState.filesTabOpen);
    setArtifactsTabOpen(nextState.artifactsTabOpen);
    setExpTabs(nextState.expTabs);
    setFileTabs(nextState.fileTabs);
    setPlanTabs(nextState.planTabs);
    setSubagentTabs(nextState.subagentTabs);
    setCodeTabs(nextState.codeTabs);
    setFilesView(nextState.filesView);
    setFilesToggled(nextState.filesToggled);
    setSelectedRunId(nextState.selectedRunId);
    setScope(nextState.scope);
    setPanelOpen(nextState.panelOpen);
    setPanelMax(nextState.panelMax);
    activeSessionIdRef.current = nextSessionId;
    setActiveSessionId(nextSessionId);
  }, []);
  const onboarded = uiState?.onboardingCompleted ?? false;
  // The spotlight tour of the workspace (Tour.tsx). Starting it normalizes
  // the layout so every tour target exists; those are the defaults, so
  // nothing needs restoring on finish/skip.
  const [tourOpen, setTourOpen] = useState(false);
  const startTour = useCallback(() => {
    setMainView("chat");
    setRailOpen(true);
    setExperimentsTabOpen(true);
    selectRightTab("experiments");
    setPanelOpen(true);
    setPanelMax(false);
    setTourOpen(true);
  }, [selectRightTab]);
  const closeTour = useCallback(async () => {
    const saved = await updateUiState({ tourCompleted: true });
    setUiState((current) => current && { ...current, tourCompleted: saved.tourCompleted });
    setTourOpen(false);
  }, []);

  // Auto-start the tour the first time the workspace is actually on screen:
  // first-run walkthrough done, a project open, projects home closed. With
  // zero projects this waits until the first one is created and opened.
  useEffect(() => {
    if (!projectId || homeOpen || !onboarded) return;
    if (uiState?.tourCompleted) return;
    startTour();
  }, [projectId, homeOpen, onboarded, startTour, uiState?.tourCompleted]);

  const projectIdRef = useRef(projectId);
  projectIdRef.current = projectId;

  const loadInitialState = useCallback(() => {
    setStartupError(null);
    setProjects(null);
    setUiState(null);
    void Promise.allSettled([listProjects(), getUiState()]).then(([projectsResult, uiStateResult]) => {
      const errors: string[] = [];
      if (projectsResult.status === "fulfilled") {
        setProjects(projectsResult.value);
        setProjectId((current) =>
          current && projectsResult.value.some((project) => project.id === current)
            ? current
            : projectsResult.value[0]?.id ?? null,
        );
      } else {
        errors.push("projects");
      }
      if (uiStateResult.status === "fulfilled") {
        persistedPreferredAgent.current = uiStateResult.value.preferredAgent;
        setUiState(uiStateResult.value);
      } else {
        errors.push("settings");
      }
      if (errors.length > 0) {
        setStartupError(`Couldn't load OpenResearch ${errors.join(" and ")}.`);
      }
    });
  }, []);
  useEffect(() => {
    loadInitialState();
  }, [loadInitialState]);

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

  // Per-project data. Harness agents spawn lazily on the first chat message.
  useEffect(() => {
    if (!projectId) return;
    const previousSessionId = activeSessionIdRef.current;
    if (previousSessionId) {
      rightPaneStatesRef.current.set(previousSessionId, currentRightPaneStateRef.current);
    }
    activeSessionIdRef.current = null;
    setActiveSessionId(null);
    // Record the visit; the resulting project.updated SSE event refreshes the
    // list's recency order.
    openProject(projectId).catch(() => {});
    setExperiments([]);
    setRuns([]);
    setArtifacts(null);
    setSelectedRunId(null);
    setExpTabs([]);
    setFileTabs([]);
    setPlanTabs([]);
    setSubagentTabs([]);
    setCodeTabs([]);
    setFilesView("files");
    setFilesToggled(new Set());
    tabHistoryRef.current = ["experiments"];
    setTabHistory(["experiments"]);
    setRightTab("experiments");
    setExperimentsTabOpen(true);
    setFilesTabOpen(false);
    setArtifactsTabOpen(false);
    // Scoping is an explicit per-project choice — don't let Current task scope
    // re-bind to whichever session ChatPanel auto-selects in the next project.
    setScope("project");
    listExperiments(projectId).then(setExperiments).catch(() => {});
    listRuns(projectId).then(setRuns).catch(() => {});
    getArtifacts(projectId).then(setArtifacts).catch(() => {});
  }, [projectId]);

  // Refetch artifacts on open and whenever the directory changes.
  const refreshArtifacts = useCallback(() => {
    const id = projectIdRef.current;
    if (id) getArtifacts(id).then(setArtifacts).catch(() => {});
  }, []);

  const openArtifactsTab = useCallback(() => {
    refreshArtifacts();
    setMainView("chat");
    setArtifactsTabOpen(true);
    selectRightTab("artifacts");
    setPanelOpen(true);
  }, [refreshArtifacts, selectRightTab]);

  const openExperimentsTab = useCallback(() => {
    setMainView("chat");
    setExperimentsTabOpen(true);
    selectRightTab("experiments");
    setPanelOpen(true);
  }, [selectRightTab]);

  // Live store updates.
  useOrxEvents({
    onRun: (run) => {
      if (run.projectId === projectIdRef.current) setRuns((cur) => upsert(cur, run));
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
  const openExperimentTab = useCallback((id: string, view: ExperimentView = "overview") => {
    const tab = { id, view };
    setExpTabs((prev) => (prev.some((t) => sameExpTab(t, tab)) ? prev : [...prev, tab]));
    selectRightTab(tab);
    setPanelOpen(true);
  }, [selectRightTab]);

  // A `<run>` evidence chip in chat opens that run's logs — the only evidence
  // channel for a metric. Run ids are globally unique, so resolve the run to its
  // experiment and open the terminal view focused on it.
  const openRunLogs = useCallback(
    (runId: string) => {
      const run = runsRef.current.find((r) => r.id === runId);
      if (!run) return;
      setSelectedRunId(runId);
      openExperimentTab(run.experimentId, "terminal");
    },
    [openExperimentTab],
  );

  const closeExperimentTab = useCallback(
    (tab: ExpViewDef) => {
      const idx = expTabs.findIndex((t) => sameExpTab(t, tab));
      if (idx === -1) return;
      setExpTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [expTabs, forgetRightTab, rightTab],
  );

  // Open a project file as a right-panel tab. `contextSessionId` is the chat
  // session (or viewed file's session) the click came from — see
  // parseFilePath for how it resolves against the reported path.
  const openFileTab = useCallback(
    (rawPath: string, contextSessionId?: string, ref?: string, line?: number, exp?: string) => {
      const project = projects?.find((p) => p.id === projectId);
      const tab = parseFilePath(
        rawPath,
        project?.repoPath,
        contextSessionId,
        project?.artifactsDir ?? project?.filesDir,
        project?.slug,
      );
      if (!tab) return;
      // A cited experiment pins the file to that node's committed branch, so the
      // tab shows (and labels) the version behind the claim. Agents cite the
      // short id (`orx` prints an 8-char prefix), so match the full id or prefix.
      const experiment = exp
        ? experimentsRef.current.find(
            (e) => e.id === exp || (exp.length >= 6 && e.id.startsWith(exp)),
          )
        : undefined;
      const effectiveRef = ref ?? experiment?.branchName;
      // A branch ref only applies to repo files; artifacts have no branch.
      if (effectiveRef && tab.source !== "artifacts") tab.ref = effectiveRef;
      if (line != null) tab.line = line;
      // Line is not part of tab identity: reopening a file at a new line reuses
      // the tab but makes the new (line-carrying) def the active one so the
      // viewer re-scrolls.
      setFileTabs((prev) => (prev.some((t) => sameFileTab(t, tab)) ? prev : [...prev, tab]));
      selectRightTab(tab);
      setPanelOpen(true);
    },
    [projects, projectId, selectRightTab],
  );

  // Chat file chips carry an optional target line (`file:line`) and cited
  // experiment (`exp`), never a branch ref — adapt to openFileTab's
  // (path, session, ref, line, exp) shape while staying referentially stable
  // for ChatPanel's memoized transcript.
  const openChatFile = useCallback(
    (path: string, sessionId?: string, line?: number, exp?: string) =>
      openFileTab(path, sessionId, undefined, line, exp),
    [openFileTab],
  );

  const closeFileTab = useCallback(
    (tab: FileViewDef) => {
      const idx = fileTabs.findIndex((t) => sameFileTab(t, tab));
      if (idx === -1) return;
      setFileTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [fileTabs, forgetRightTab, rightTab],
  );

  // Open a proposed plan as a right-panel tab (the chat plan strip's "View
  // plan"). One tab per plan card; re-opening the same card refreshes its
  // text (a revised plan re-uses the strip but is a new promptId → new tab).
  const openPlanTab = useCallback((plan: string, sessionId: string, promptId: string) => {
    const tab: PlanViewDef = { kind: "plan", sessionId, promptId, plan };
    setPlanTabs((prev) => {
      const idx = prev.findIndex((t) => t.promptId === promptId);
      if (idx === -1) return [...prev, tab];
      const next = prev.slice();
      next[idx] = tab;
      return next;
    });
    selectRightTab(tab);
    setPanelOpen(true);
  }, [selectRightTab]);

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
  const openSubagentTab = useCallback((sessionId: string, spawnPartId: string) => {
    const tab: SubagentViewDef = { kind: "subagent", sessionId, spawnPartId };
    setSubagentTabs((prev) =>
      prev.some((t) => t.spawnPartId === spawnPartId) ? prev : [...prev, tab],
    );
    selectRightTab(tab);
    setPanelOpen(true);
  }, [selectRightTab]);

  const closeSubagentTab = useCallback(
    (tab: SubagentViewDef) => {
      const idx = subagentTabs.findIndex((t) => t.spawnPartId === tab.spawnPartId);
      if (idx === -1) return;
      setSubagentTabs((prev) => prev.filter((_, i) => i !== idx));
      forgetRightTab(tab, rightTabKey(rightTab) === rightTabKey(tab));
    },
    [forgetRightTab, rightTab, subagentTabs],
  );

  // One Git-backed code tab per branch. Reopening the same branch focuses it
  // at the requested subview; another branch gets its own tab.
  const openCodeTabForExperiment = useCallback(
    (experimentId: string, branch: string, view: CodeView = "files") => {
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
      selectRightTab(opened);
      setPanelOpen(true);
    },
    [selectRightTab],
  );

  const updateCodeTab = useCallback(
    (tab: CodeTabDef, patch: Partial<Omit<CodeTabDef, "code" | "branch">>) => {
      setCodeTabs((prev) =>
        prev.map((item) => (sameCodeTab(item, tab) ? { ...item, ...patch } : item)),
      );
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
    setMainView("chat");
    setFilesTabOpen(true);
    selectRightTab("files");
    setPanelOpen(true);
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
    e.currentTarget.setPointerCapture(e.pointerId);
    const prevUserSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    const onMove = (ev: PointerEvent) => {
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
      try {
        localStorage.setItem(PANEL_WIDTH_KEY, String(clamped));
      } catch {
        // best-effort persistence
      }
    };
    const stop = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      document.body.style.userSelect = prevUserSelect;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  };

  const onProjectCreated = (project: Project, publicationError: string | null) => {
    setProjects((cur) => (cur ? upsert(cur, project) : [project]));
    setProjectId(project.id);
    setHomeOpen(false);
    if (publicationError) {
      setGithubPublicationError({ projectId: project.id, message: publicationError });
      setMainView("git");
    }
  };

  const onProjectDeleted = (id: string) => {
    setProjects((cur) => (cur ? cur.filter((p) => p.id !== id) : cur));
    if (projectId === id) setProjectId(null);
  };

  const expTab =
    typeof rightTab === "object" && "id" in rightTab ? rightTab : null;
  const fileTab = typeof rightTab === "object" && "path" in rightTab ? rightTab : null;
  // PlanViewDef and SubagentViewDef both carry `kind`; discriminate on its value.
  const planTab =
    typeof rightTab === "object" && "kind" in rightTab && rightTab.kind === "plan"
      ? rightTab
      : null;
  const subagentTab =
    typeof rightTab === "object" && "kind" in rightTab && rightTab.kind === "subagent"
      ? rightTab
      : null;
  const requestedCodeTab =
    typeof rightTab === "object" && "code" in rightTab ? rightTab : null;
  const codeTab = requestedCodeTab
    ? (codeTabs.find((tab) => sameCodeTab(tab, requestedCodeTab)) ?? null)
    : null;
  const activeProject = projects?.find((p) => p.id === projectId) ?? null;
  const tabExperiment = expTab ? (experiments.find((e) => e.id === expTab.id) ?? null) : null;
  const codeExperiment = codeTab
    ? (experiments.find((experiment) => experiment.id === codeTab.experimentId) ?? null)
    : null;

  if (startupError) {
    return (
      <div className="app flex flex-col h-full">
        <div className={EMPTY_STATE_CLASS_NAME}>
          <p>{startupError}</p>
          <button className={PRIMARY_BUTTON_CLASS_NAME} onClick={loadInitialState}>Retry</button>
        </div>
      </div>
    );
  }

  if (projects === null || uiState === null) {
    return (
      <div className="app flex flex-col h-full">
        <div className={EMPTY_STATE_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} />
        </div>
      </div>
    );
  }

  // First boot: the walkthrough installs and opens the embedded demo project.
  if (projects.length === 0) {
    return (
      <div className="app flex flex-col h-full">
        {onboarded ? (
          <ProjectsHome
            projects={projects}
            onOpen={setProjectId}
            onCreated={onProjectCreated}
            onDeleted={onProjectDeleted}
          />
        ) : (
          <Onboarding
            preferredAgent={uiState.preferredAgent}
            onDone={(project, selection) => {
              clearReadDemoSessions();
              persistedPreferredAgent.current = selection;
              setProjects([project]);
              setProjectId(project.id);
              setUiState((current) => ({
                ...(current ?? { tourCompleted: false }),
                onboardingCompleted: true,
                preferredAgent: selection,
              }));
            }}
          />
        )}
      </div>
    );
  }

  const railHeader = (
    <RailHeader
      projectName={projects.find((p) => p.id === projectId)?.name ?? ""}
      onHome={() => setHomeOpen(true)}
      onNewProject={() => setNewProjectOpen(true)}
      onRepository={() => setMainView("git")}
      onCollapse={() => setRailOpen(false)}
    />
  );

  return (
    <div className="app flex flex-col h-full">
      {homeOpen ? (
        <ProjectsHome
          projects={projects}
          onOpen={(id) => {
            setProjectId(id);
            setHomeOpen(false);
          }}
          onCreated={onProjectCreated}
          onDeleted={onProjectDeleted}
        />
      ) : (
      <div className="app-body flex flex-1 min-h-0 py-0 px-3.5">
        {projectId && (
          <ChatPanel
            projectId={projectId}
            projectName={activeProject?.name ?? ""}
            paperId={projects.find((p) => p.id === projectId)?.paperId}
            railHeader={railHeader}
            railOpen={railOpen}
            onShowRail={() => setRailOpen(true)}
            mainView={mainView}
            onSelectMainView={setMainView}
            experimentsActive={
              mainView === "chat" && panelOpen && rightTab === "experiments"
            }
            filesActive={mainView === "chat" && panelOpen && rightTab === "files"}
            artifactsActive={mainView === "chat" && panelOpen && rightTab === "artifacts"}
            onOpenExperiments={openExperimentsTab}
            onOpenArtifacts={openArtifactsTab}
            onOpenFile={openChatFile}
            onOpenRun={openRunLogs}
            onOpenPlan={openPlanTab}
            onOpenSubagent={openSubagentTab}
            onOpenWorktree={openWorktreeTab}
            onStartTour={startTour}
            onActiveSessionChange={onActiveSessionChange}
            preferredAgent={uiState.preferredAgent}
            onPreferredAgentChange={persistPreferredAgent}
          >
            {mainView !== "chat" && (
              <SettingsView
                tab={mainView}
                project={activeProject}
                githubPublicationError={
                  githubPublicationError && githubPublicationError.projectId === activeProject?.id
                    ? githubPublicationError.message
                    : null
                }
                onProjectUpdate={(project) => {
                  setProjects((current) => (current ? upsert(current, project) : [project]));
                  if (project.githubEnabled) setGithubPublicationError(null);
                }}
                onSelectTab={setMainView}
              />
            )}
          </ChatPanel>
        )}
        {mainView === "chat" && panelOpen && (
        <aside
          className={`right-pane relative shrink-0 min-w-0 flex flex-col mt-2.5 mr-0 mb-2.5 ml-3.5 bg-canvas [&.max]:fixed [&.max]:inset-2.5 [&.max]:m-0 [&.max]:z-60 [&.max]:shadow-[0_12px_40px_color-mix(in_oklab,_var(--text)_22%,_transparent)] floating-panel border border-border rounded-lg shadow-[0_6px_24px_color-mix(in_oklab,_var(--text)_5%,_transparent),_0_1px_4px_color-mix(in_oklab,_var(--text)_4%,_transparent)] overflow-hidden ${panelMax ? "max" : ""}`}
          style={panelMax ? undefined : { width: panelWidth }}
          data-onboarding="experiments"
        >
          {!panelMax && <div className="panel-resizer absolute left-0 top-0 bottom-0 w-1.5 cursor-col-resize z-30 [&:hover]:bg-[color-mix(in_oklab,_var(--text)_12%,_transparent)] [&:active]:bg-[color-mix(in_oklab,_var(--text)_12%,_transparent)]" onPointerDown={resizePanel} />}
          <div className="tabs flex items-end gap-0 pt-1 pr-1.5 pb-0 pl-2 h-10 border-b border-b-border bg-canvas shrink-0">
            <div className="tab-strip flex items-end gap-0.5 flex-1 min-w-0 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              {artifactsTabOpen && (
                <ClosableTab
                  active={rightTab === "artifacts"}
                  label="Artifacts"
                  icon={<Package size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab("artifacts")}
                  onClose={() => closeHomeTab("artifacts")}
                />
              )}
              {experimentsTabOpen && (
                <ClosableTab
                  active={rightTab === "experiments"}
                  label="Experiments"
                  icon={<FlaskConical size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab("experiments")}
                  onClose={() => closeHomeTab("experiments")}
                />
              )}
              {filesTabOpen && (
                <ClosableTab
                  active={rightTab === "files"}
                  label="Files"
                  icon={<FolderOpen size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab("files")}
                  onClose={() => closeHomeTab("files")}
                />
              )}
              {expTabs.map((t) => {
                const exp = experiments.find((e) => e.id === t.id);
                return (
                  <ClosableTab
                    key={`${t.id}:${t.view}`}
                    active={expTab !== null && sameExpTab(expTab, t)}
                    label={exp ? exp.title || exp.slug : "…"}
                    icon={
                      t.view === "overview" ? (
                        <ChartSpline size={12} style={{ flexShrink: 0 }} />
                      ) : (
                        <Terminal size={12} style={{ flexShrink: 0 }} />
                      )
                    }
                    onSelect={() => selectRightTab(t)}
                    onClose={() => closeExperimentTab(t)}
                  />
                );
              })}
              {fileTabs.map((t) => (
                <ClosableTab
                  key={`file:${fileTabKey(t)}`}
                  active={fileTab !== null && sameFileTab(fileTab, t)}
                  label={t.path.split("/").pop() || t.path}
                  icon={<FileCode size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab(t)}
                  onClose={() => closeFileTab(t)}
                />
              ))}
              {planTabs.map((t) => (
                <ClosableTab
                  key={`plan:${t.promptId}`}
                  active={planTab !== null && planTab.promptId === t.promptId}
                  label="Plan"
                  icon={<ScrollText size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab(t)}
                  onClose={() => closePlanTab(t)}
                />
              ))}
              {subagentTabs.map((t) => (
                <ClosableTab
                  key={`subagent:${t.spawnPartId}`}
                  active={subagentTab !== null && subagentTab.spawnPartId === t.spawnPartId}
                  label="Sub-agent"
                  icon={<Users size={12} style={{ flexShrink: 0 }} />}
                  onSelect={() => selectRightTab(t)}
                  onClose={() => closeSubagentTab(t)}
                />
              ))}
              {codeTabs.map((tab) => {
                const experiment = experiments.find((item) => item.id === tab.experimentId);
                return (
                  <ClosableTab
                    key={`code:${tab.branch}`}
                    active={codeTab !== null && sameCodeTab(codeTab, tab)}
                    label={experiment?.slug ?? tab.branch}
                    icon={<FolderOpen size={12} style={{ flexShrink: 0 }} />}
                    onSelect={() => selectRightTab(tab)}
                    onClose={() => closeCodeTab(tab)}
                  />
                );
              })}
            </div>
            <div className="panel-controls inline-flex items-center gap-0.5 self-center py-0 px-1.5 shrink-0">
              <button
                className={ICON_BUTTON_CLASS_NAME}
                title={panelMax ? "Restore panel" : "Expand panel"}
                aria-label={panelMax ? "Restore panel" : "Expand panel"}
                onClick={() => setPanelMax((m) => !m)}
              >
                {panelMax ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
              </button>
              <button
                className={ICON_BUTTON_CLASS_NAME}
                title="Close panel"
                aria-label="Close panel"
                onClick={() => {
                  setPanelOpen(false);
                  setPanelMax(false);
                }}
              >
                <X size={14} />
              </button>
            </div>
          </div>
          {rightTab === "artifacts" ? (
            <div className={TAB_BODY_CLASS_NAME}>
              {activeProject && (
                <ArtifactsTab
                  key={activeProject.id}
                  project={activeProject}
                  artifacts={artifacts}
                  onChanged={refreshArtifacts}
                  onOpenStorage={() => setMainView("storage")}
                />
              )}
            </div>
          ) : rightTab === "experiments" ? (
            <div className={TAB_BODY_CLASS_NAME}>
              <div className={`pane-toolbar flex items-center gap-2 flex-wrap pt-2.5 px-3 pb-0 shrink-0 [&.table-view]:bg-background [&.table-view]:pb-3${view === "table" ? " table-view" : ""}`}>
                <span style={{ flex: 1 }} />
                <div className="experiments-toolbar-controls inline-flex items-center gap-[5px]">
                  <div className="option-picker relative inline-flex" ref={scopeMenuRef}>
                    <button
                      ref={scopeTriggerRef}
                      className={`${ICON_BUTTON_BASE_CLASS_NAME} experiment-scope-trigger w-6.5 h-6.5 rounded-sm${effectiveScope === "agent" ? " active" : ""}`}
                      title={`Experiment filter: ${effectiveScope === "agent" ? "Current task" : "Entire project"}`}
                      aria-label="Filter experiments"
                      aria-expanded={scopeMenuOpen}
                      onClick={() => setScopeMenuOpen((open) => !open)}
                    >
                      <Filter size={16} strokeWidth={2.5} />
                    </button>
                    {scopeMenuOpen && (
                      <div className="option-menu absolute bottom-[calc(100%_+_8px)] left-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:left-auto [&.align-right]:right-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:left-auto [&.session-menu]:right-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down align-right experiment-scope-menu [&_.model-item]:whitespace-nowrap [&_.model-item:disabled]:text-muted [&_.model-item:disabled]:cursor-default [&_.model-item:disabled:hover]:bg-transparent">
                        <button
                          className={MODEL_ITEM_CLASS_NAME}
                          aria-pressed={effectiveScope === "agent"}
                          disabled={!activeSessionId || !allExperimentsAttributed}
                          title={
                            !activeSessionId
                              ? "Open a task to filter to its experiments"
                              : !allExperimentsAttributed
                                ? "Current task filtering is unavailable for unattributed experiments"
                                : undefined
                          }
                          onClick={() => {
                            setScope("agent");
                            setScopeMenuOpen(false);
                          }}
                        >
                          <span>Current task</span>
                          {effectiveScope === "agent" && <Check size={13} />}
                        </button>
                        <button
                          className={MODEL_ITEM_CLASS_NAME}
                          aria-pressed={effectiveScope === "project"}
                          onClick={() => {
                            setScope("project");
                            setScopeMenuOpen(false);
                          }}
                        >
                          <span>Entire project</span>
                          {effectiveScope === "project" && <Check size={13} />}
                        </button>
                      </div>
                    )}
                  </div>
                  <div
                    className="seg inline-flex items-center gap-0.5 rounded-md bg-[color-mix(in_oklab,_var(--text)_10%,_transparent)] [&_button]:font-semibold [&_button]:text-text [&_button]:rounded-sm [&_button:not(:disabled):hover]:text-text [&_button.active]:bg-background [&_button.active]:shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_25%,_transparent)] [&_button:disabled]:text-muted [&_button:disabled]:cursor-default experiments-view-toggle p-0.5 [&_button]:py-0.5 [&_button]:px-2 [&_button]:text-sm"
                    role="group"
                    aria-label="Experiment view"
                  >
                    <button
                      className={view === "table" ? "active" : ""}
                      aria-pressed={view === "table"}
                      onClick={() => setView("table")}
                    >
                      Table
                    </button>
                    <button
                      className={view === "tree" ? "active" : ""}
                      aria-pressed={view === "tree"}
                      onClick={() => setView("tree")}
                    >
                      Tree
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
                        ? "No experiments from the current task yet. Switch to Entire project to see all experiments."
                        : undefined
                    }
                    experiments={scopedExperiments}
                    onOpen={(experiment) => {
                      openExperimentTab(experiment.id, "overview");
                    }}
                    onOpenLogs={(experimentId, runId) => {
                      setSelectedRunId(runId);
                      openExperimentTab(experimentId, "terminal");
                    }}
                    onOpenCode={(experimentId) => {
                      const experiment = experiments.find((item) => item.id === experimentId);
                      if (experiment)
                        openCodeTabForExperiment(experiment.id, experiment.branchName, "files");
                    }}
                    onCancel={cancelRun}
                  />
                )}
              </div>
            </div>
          ) : rightTab === "files" ? (
            <div className={TAB_BODY_CLASS_NAME}>
              {activeProject ? (
                <WorktreeTab
                  key={`files:${activeSessionId ?? `project:${activeProject.id}`}`}
                  sessionId={activeSessionId ?? undefined}
                  project={activeProject}
                  view={filesView}
                  toggled={filesToggled}
                  onViewChange={setFilesView}
                  onToggledChange={setFilesToggled}
                  onOpenFile={openFileTab}
                />
              ) : (
                <div className="code-tab flex flex-col h-full min-h-0 wt-tab">
                  <div className={CODE_TAB_BODY_CLASS_NAME}>
                    <div className="wt-empty flex flex-col items-center gap-2.5 py-12 px-6 text-center text-muted [&_>_svg]:text-subtext [&_p]:m-0 [&_p]:max-w-80 [&_p]:text-sm">
                      <FolderGit2 size={22} />
                      <p>Select a project to browse its files.</p>
                    </div>
                  </div>
                </div>
              )}
            </div>
          ) : fileTab ? (
            <div className={TAB_BODY_CLASS_NAME}>
              {projectId && (
                <FileViewer
                  key={fileTabKey(fileTab)}
                  projectId={projectId}
                  path={fileTab.path}
                  source={fileTab.source}
                  sessionId={fileTab.sessionId}
                  gitRef={fileTab.ref}
                  line={fileTab.line}
                  branchLabel={fileBranchLabel(fileTab, activeProject?.baselineBranch)}
                  onOpenFile={openFileTab}
                />
              )}
            </div>
          ) : planTab ? (
            <div className={TAB_BODY_CLASS_NAME}>
              {/* The plan markdown is already client-side — render directly,
                  file links resolve against the plan's session worktree. */}
              <div className="pane-content flex-1 min-h-0 relative plan-tab-content overflow-y-auto bg-background py-4.5 px-6 [&_.md]:max-w-readable">
                <Md
                  text={planTab.plan}
                  onOpenFile={(path) => openFileTab(path, planTab.sessionId)}
                />
              </div>
            </div>
          ) : subagentTab ? (
            <SubagentTab
              // Remount per spawn part so the seed + subscription reset cleanly.
              key={subagentTab.spawnPartId}
              sessionId={subagentTab.sessionId}
              spawnPartId={subagentTab.spawnPartId}
              onOpenFile={(path) => openFileTab(path, subagentTab.sessionId)}
              onOpenRun={openRunLogs}
              onOpenSubagent={(pid) => openSubagentTab(subagentTab.sessionId, pid)}
            />
          ) : codeTab ? (
            <div className={TAB_BODY_CLASS_NAME}>
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
                  onOpenFile={openFileTab}
                />
              )}
            </div>
          ) : (
            <div className={TAB_BODY_CLASS_NAME}>
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
                  onOpenView={(view, runId) => {
                    if (runId) setSelectedRunId(runId);
                    openExperimentTab(tabExperiment.id, view);
                  }}
                  onOpenCode={(view) =>
                    openCodeTabForExperiment(
                      tabExperiment.id,
                      tabExperiment.branchName,
                      view,
                    )
                  }
                />
              )}
            </div>
          )}
        </aside>
        )}
      </div>
      )}
      {newProjectOpen && (
        <NewProjectDialog
          onClose={() => setNewProjectOpen(false)}
          onCreated={(project, publicationError) => {
            setNewProjectOpen(false);
            onProjectCreated(project, publicationError);
          }}
        />
      )}
      {tourOpen && !homeOpen && projectId && <Tour onClose={closeTour} />}
    </div>
  );
}
