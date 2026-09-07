import type { ExperimentView } from "./components/DetailDrawer";
import type { CodeView } from "./components/CodeTab";
import type { WorktreeView } from "./components/WorktreeTab";
import { DEMO_MAIN_SESSION_ID, DEMO_FIGURE_SESSION_ID, DEMO_LITERATURE_SESSION_ID, DEMO_OVERVIEW_ARTIFACT } from "./api";
import type { Pane, TaskWorkspace } from "./workspaceState";

export function tabPane(tab: RightTab, runId?: string | null): Pane {
  if (typeof tab === "string") return { kind: "home", view: tab };
  if ("code" in tab) return { kind: "code", experimentId: tab.experimentId, branch: tab.branch, view: tab.view };
  if ("kind" in tab) return tab.kind === "plan"
    ? { kind: "plan", sessionId: tab.sessionId, promptId: tab.promptId }
    : { kind: "subagent", sessionId: tab.sessionId, spawnPartId: tab.spawnPartId };
  if ("path" in tab) return { kind: "file", path: tab.path, source: tab.source, sessionId: tab.sessionId, ref: tab.ref, line: tab.line, branchLabel: tab.branchLabel };
  const selectedRun = runId === undefined ? tab.runId : runId;
  return { kind: "experiment", experimentId: tab.id, view: tab.view, ...(selectedRun ? { runId: selectedRun } : {}) };
}

export function paneTab(pane: Pane): RightTab {
  switch (pane.kind) {
    case "home": return pane.view;
    case "experiment": return { id: pane.experimentId, view: pane.view, ...(pane.runId ? { runId: pane.runId } : {}) };
    case "file": return { path: pane.path, source: pane.source, sessionId: pane.sessionId, ref: pane.ref, line: pane.line, branchLabel: pane.branchLabel };
    case "code": return { code: true, experimentId: pane.experimentId, branch: pane.branch, view: pane.view, toggled: new Set() };
    case "plan": return { kind: "plan", sessionId: pane.sessionId, promptId: pane.promptId, plan: "" };
    case "subagent": return { kind: "subagent", sessionId: pane.sessionId, spawnPartId: pane.spawnPartId };
  }
}

export function rememberWorkspace(state: RightPaneSessionState, scroll: TaskWorkspace["scroll"], sourceModes: TaskWorkspace["sourceModes"]): TaskWorkspace {
  const home: RightTab[] = [];
  if (state.filesTabOpen) home.push("files");
  if (state.artifactsTabOpen) home.push("artifacts");
  if (state.experimentsTabOpen) home.push("experiments");
  const content = [...state.expTabs, ...state.fileTabs, ...state.planTabs, ...state.subagentTabs, ...state.codeTabs];
  const byKey = new Map(content.map((tab) => [rightTabKey(tab), tab]));
  const ordered = state.contentTabOrder.flatMap((key) => { const tab = byKey.get(key); return tab ? [tab] : []; });
  const activeKey = rightTabKey(state.rightTab);
  const tabs = [...home, ...ordered].map((tab) => tabPane(tab, rightTabKey(tab) === activeKey ? state.selectedRunId : undefined));
  return { tabs, active: state.panelOpen ? tabPane(state.rightTab, state.selectedRunId) : null,
    previewKey: state.previewTab ? rightTabKey(state.previewTab) : null,
    history: state.tabHistory.map(rightTabKey),
    expanded: Object.fromEntries([["files", [...state.filesToggled]], ...state.codeTabs.map((tab) => [rightTabKey(tab), [...tab.toggled]])]),
    scroll, sourceModes, filesView: state.filesView, scope: state.scope, panelMax: state.panelMax };
}

export function restoreWorkspace(saved: TaskWorkspace | undefined, pane: Pane | undefined): RightPaneSessionState {
  const state = initialRightPaneSessionState();
  if (saved) {
    state.filesView = saved.filesView;
    state.filesToggled = new Set(saved.expanded.files ?? []);
    state.scope = saved.scope;
    state.panelMax = saved.panelMax;
  }
  const tabs = [...(saved?.tabs ?? [])];
  if (pane) {
    const index = tabs.findIndex((item) => rightTabKey(paneTab(item)) === rightTabKey(paneTab(pane)));
    if (index === -1) tabs.push(pane);
    else tabs[index] = pane;
  }
  for (const item of tabs) {
    const tab = paneTab(item);
    if (typeof tab === "string") {
      if (tab === "experiments") state.experimentsTabOpen = true;
      if (tab === "files") state.filesTabOpen = true;
      if (tab === "artifacts") state.artifactsTabOpen = true;
      continue;
    }
    if ("code" in tab) { tab.toggled = new Set(saved?.expanded[rightTabKey(tab)] ?? []); state.codeTabs.push(tab); }
    else if ("path" in tab) state.fileTabs.push(tab);
    else if ("kind" in tab) {
      if (tab.kind === "plan") state.planTabs.push(tab);
      else state.subagentTabs.push(tab);
    } else state.expTabs.push(tab);
    state.contentTabOrder.push(rightTabKey(tab));
  }
  const byKey = new Map(tabs.map((item) => { const tab = paneTab(item); return [rightTabKey(tab), tab]; }));
  state.tabHistory = (saved?.history ?? []).flatMap((key) => { const tab = byKey.get(key); return tab ? [tab] : []; });
  state.previewTab = saved?.previewKey ? byKey.get(saved.previewKey) ?? null : null;
  state.rightTab = pane ? paneTab(pane) : saved?.active ? paneTab(saved.active) : "experiments";
  state.panelOpen = pane !== undefined;
  state.selectedRunId = pane?.kind === "experiment" ? pane.runId ?? null : null;
  return applyPane(state, pane);
}

/** An experiment view open as a right-panel tab. */
export interface ExpViewDef {
  id: string;
  view: ExperimentView;
  runId?: string;
}

export const sameExpTab = (a: ExpViewDef, b: ExpViewDef) => a.id === b.id && a.view === b.view;

/** A project file open as a right-panel tab (clicked in chat tool rows or the
 * code browser). */
export interface FileViewDef {
  path: string;
  /** Which backend serves this file. Absent/"repo" → the repo `/file`
   * endpoint (worktree/clone/branch), falling back to artifacts when a
   * non-ref path misses the checkout; "artifacts" → the project's durable
   * output directory through the compatibility `/files/file` endpoint;
   * "abs" → an absolute path on disk outside both (the `/files/abs`
   * endpoint), for files an agent references anywhere on the machine. */
  source?: "repo" | "artifacts" | "abs";
  /** Chat session whose worktree holds the file (absent → hub clone).
   * Artifact and absolute-path tabs never carry this. */
  sessionId?: string;
  /** Branch whose committed copy to show (code browser in branch mode);
   * overrides the live checkout. */
  ref?: string;
  /** Branch to show in the header chip when the file is read from a checkout
   * (no `ref`) whose branch isn't the baseline — e.g. an experiment's worktree.
   * Display-only, so it's kept out of tab identity. */
  branchLabel?: string;
  /** 1-based line to scroll to and highlight on open (from a `file:line`
   * evidence chip). Not part of tab identity — reopening at a new line updates
   * the same tab. */
  line?: number;
  /** One-shot generation for explicit line navigation; omitted on stored tabs. */
  lineScrollRequest?: number;
}

export const sameFileTab = (a: FileViewDef, b: FileViewDef) =>
  a.path === b.path &&
  (a.source ?? "repo") === (b.source ?? "repo") &&
  a.sessionId === b.sessionId &&
  a.ref === b.ref;

export const fileTabKey = (t: FileViewDef) =>
  `${t.source ?? "repo"}:${t.sessionId ?? ""}:${t.ref ?? ""}:${t.path}`;

export const fileScrollKey = (projectId: string, ownerSessionId: string | null, tab: FileViewDef) =>
  `${projectId}:${ownerSessionId ?? ""}:${fileTabKey(tab)}`;

export const persistentFileTab = (tab: FileViewDef): FileViewDef => ({
  ...tab,
  lineScrollRequest: undefined,
});

export function persistentRightTab(tab: RightTab): RightTab {
  return typeof tab === "object" && "path" in tab
    ? persistentFileTab(tab)
    : tab;
}

/** A proposed plan open as a right-panel tab (from the chat plan strip/card).
 * The markdown is already client-side (it rode the prompt part), so the tab
 * renders it directly — no fetch. Deliberately has neither a `view` nor a
 * `path` field: the other tab kinds discriminate on those. */
export interface PlanViewDef {
  kind: "plan";
  sessionId: string;
  /** The prompt part the plan came from — one tab per plan card. */
  promptId: string;
  plan: string;
}

/** A sub-agent's transcript, opened from a chat spawn row's "view" button. One
 * tab per spawn part; its parts stream live off the session's chat message. */
export interface SubagentViewDef {
  kind: "subagent";
  sessionId: string;
  /** The `subagent` spawn part whose `children` are the sub-agent transcript. */
  spawnPartId: string;
  /** The spawn row's activity label at open time — the tab title. */
  label?: string;
}

/** One committed code-browser tab per experiment branch. Source, selected
 * view, and expansion state live here so they survive tab switches. */
export interface CodeTabDef {
  code: true;
  experimentId: string;
  branch: string;
  view: CodeView;
  /** Dirs the user flipped away from their depth default. */
  toggled: ReadonlySet<string>;
}

export const sameCodeTab = (a: CodeTabDef, b: CodeTabDef) => a.branch === b.branch;

export type RightTab =
  | "experiments"
  | "files"
  | "artifacts"
  | ExpViewDef
  | FileViewDef
  | PlanViewDef
  | SubagentViewDef
  | CodeTabDef;

export type ContentTab = Exclude<RightTab, string>;

export function rightTabKey(tab: RightTab): string {
  if (typeof tab === "string") return `home:${tab}`;
  if ("code" in tab) return `code:${tab.branch}`;
  if ("kind" in tab) {
    return tab.kind === "plan" ? `plan:${tab.promptId}` : `subagent:${tab.spawnPartId}`;
  }
  if ("path" in tab) return `file:${fileTabKey(tab)}`;
  return `experiment:${tab.id}:${tab.view}`;
}

/** Drop `key`'s tab from one strip list, keeping the array identity (and so the
 * effects keyed on it) when the tab doesn't live in this list. */
export function withoutTab<T extends RightTab>(tabs: T[], key: string): T[] {
  const next = tabs.filter((tab) => rightTabKey(tab) !== key);
  return next.length === tabs.length ? tabs : next;
}

export function isPresent<T>(value: T | undefined): value is T {
  return value !== undefined;
}

export interface RightPaneSessionState {
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
  /** Stable strip order for content tabs; home tabs keep their fixed leading slots. */
  contentTabOrder: string[];
  /** The reusable preview tab, replaced by the next preview open. */
  previewTab: RightTab | null;
  filesView: WorktreeView;
  filesToggled: ReadonlySet<string>;
  selectedRunId: string | null;
  scope: "agent" | "project";
  panelOpen: boolean;
  panelMax: boolean;
}

export function initialRightPaneSessionState(
  sessionId?: string,
  openDemoOverview = false,
): RightPaneSessionState {
  const initial: RightPaneSessionState = {
    rightTab: "experiments",
    tabHistory: [],
    experimentsTabOpen: false,
    filesTabOpen: false,
    artifactsTabOpen: false,
    expTabs: [],
    fileTabs: [],
    planTabs: [],
    subagentTabs: [],
    codeTabs: [],
    contentTabOrder: [],
    previewTab: null,
    filesView: "files",
    filesToggled: new Set(),
    selectedRunId: null,
    scope: "project",
    panelOpen: false,
    panelMax: false,
  };
  if (sessionId === DEMO_MAIN_SESSION_ID && openDemoOverview) {
    const demoOverviewTab: FileViewDef = {
      path: DEMO_OVERVIEW_ARTIFACT,
      source: "artifacts",
    };
    // First demo open leads with the experiments tab so the idle follow-ups
    // are visible next to the prefilled prompt that runs one of them.
    const experimentsTab: RightTab = "experiments";
    return {
      ...initial,
      rightTab: experimentsTab,
      tabHistory: [demoOverviewTab, experimentsTab],
      experimentsTabOpen: true,
      fileTabs: [demoOverviewTab],
      contentTabOrder: [rightTabKey(demoOverviewTab)],
      panelOpen: true,
    };
  }
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
      fileTabs,
      contentTabOrder: fileTabs.map(rightTabKey),
      panelOpen: true,
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
      fileTabs,
      contentTabOrder: fileTabs.map(rightTabKey),
      panelOpen: true,
    };
  }
  return initial;
}

export function defaultTaskWorkspace(sessionId: string | undefined, openDemoOverview: boolean): TaskWorkspace | undefined {
  const state = initialRightPaneSessionState(sessionId, openDemoOverview);
  return state.panelOpen ? rememberWorkspace(state, {}, {}) : undefined;
}

export function applyPane(state: RightPaneSessionState, pane: Pane | undefined): RightPaneSessionState {
  if (!pane) return state;
  const tab = paneTab(pane);
  const key = rightTabKey(tab);
  const existing = [...state.expTabs, ...state.fileTabs, ...state.codeTabs, ...state.planTabs, ...state.subagentTabs].find((item) => rightTabKey(item) === key);
  const update = <T extends ContentTab>(tabs: T[], target: T): T[] => {
    const index = tabs.findIndex((item) => rightTabKey(item) === key);
    if (index < 0) return [...tabs, target];
    if (JSON.stringify(tabPane(tabs[index])) === JSON.stringify(tabPane(target))) return tabs;
    return tabs.map((item, i) => i === index ? { ...item, ...target } : item);
  };
  const next = { ...state };
  if (typeof tab === "string") {
    if (tab === "files") next.filesTabOpen = true;
    else if (tab === "artifacts") next.artifactsTabOpen = true;
    else next.experimentsTabOpen = true;
  } else {
    if ("path" in tab) next.fileTabs = update(state.fileTabs, tab);
    else if ("id" in tab) next.expTabs = update(state.expTabs, { ...tab, runId: pane.kind === "experiment" ? pane.runId : undefined });
    else if ("code" in tab) next.codeTabs = update(state.codeTabs, { ...tab, toggled: existing && "code" in existing ? existing.toggled : tab.toggled });
    else if (tab.kind === "plan") next.planTabs = update(state.planTabs, tab);
    else next.subagentTabs = update(state.subagentTabs, tab);
    if (!state.contentTabOrder.includes(key)) next.contentTabOrder = [...state.contentTabOrder, key];
  }
  const last = state.tabHistory.at(-1);
  if (last && rightTabKey(last) === key && JSON.stringify(tabPane(last)) === JSON.stringify(tabPane(tab))) return next;
  next.tabHistory = [...state.tabHistory.filter((item) => rightTabKey(item) !== key), tab];
  return next;
}
