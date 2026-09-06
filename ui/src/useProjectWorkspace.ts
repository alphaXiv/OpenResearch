import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type MutableRefObject } from "react";
import { getProjectUiState, isDemoProjectId, saveProjectUiState } from "./api";
import { createWorkspaceWriter, emptyProjectWorkspace, getTaskWorkspace, safeLocation, type Pane, type ProjectWorkspace, type TaskWorkspace } from "./workspaceState";
import { applyPane, defaultTaskWorkspace, paneTab, rememberWorkspace, restoreWorkspace, rightTabKey, fileScrollKey, type RightPaneSessionState } from "./workspaceTabs";

let epoch = 0;
const writers = new Map<string, ReturnType<typeof createWorkspaceWriter<ProjectWorkspace>>>();
const saveErrors = new Map<string, string>();
const errorListeners = new Set<() => void>();
const notifyErrors = () => { for (const listener of errorListeners) listener(); };
const subscribeErrors = (listener: () => void) => { errorListeners.add(listener); return () => { errorListeners.delete(listener); }; };
const newTaskPromotions = new Map<string, string>();
const projectCache = new Map<string, ProjectWorkspace>();
export const getCachedProjectWorkspace = (projectId: string) => projectCache.get(projectId);
export function clearProjectWorkspaceCache() {
  epoch++;
  projectCache.clear();
  newTaskPromotions.clear();
  writers.clear();
  saveErrors.clear();
  notifyErrors();
}
export function inheritNewTaskWorkspace(projectId: string, sessionId: string) {
  const current = projectCache.get(projectId);
  if (!current?.tasks.new || getTaskWorkspace(current, sessionId)) return;
  const source = current.tasks.new;
  const remap = <T>(values: Record<string, T>) => Object.fromEntries(source.tabs.flatMap((pane) => {
    if (pane.kind !== "file") return [];
    const tab = paneTab(pane);
    if (typeof tab === "string" || !("path" in tab)) return [];
    const oldKey = fileScrollKey(projectId, null, tab);
    return oldKey in values ? [[fileScrollKey(projectId, sessionId, tab), values[oldKey]]] : [];
  }));
  const tasks = { ...current.tasks, [sessionId]: { ...source, scroll: remap(source.scroll), sourceModes: remap(source.sourceModes) } };
  delete tasks.new;
  newTaskPromotions.set(projectId, sessionId);
  projectCache.set(projectId, { ...current, tasks });
}

function writerFor(id: string) {
  let writer = writers.get(id);
  if (!writer) {
    const visit = epoch;
    writer = createWorkspaceWriter<ProjectWorkspace>(async (value, unloading) => {
      if (visit !== epoch) return;
      await saveProjectUiState(id, value, unloading);
      if (visit === epoch && saveErrors.delete(id)) notifyErrors();
    }, (error) => {
      if (visit !== epoch) return;
      saveErrors.set(id, error instanceof Error ? error.message : String(error));
      notifyErrors();
    });
    writers.set(id, writer);
  }
  return writer;
}

function save(id: string, location: string, key?: string, task?: TaskWorkspace) {
  const previous = projectCache.get(id);
  if (!previous || (key === "new" && newTaskPromotions.has(id))) return;
  const lastLocation = safeLocation(location) ?? previous.lastLocation;
  const lastTaskId = key ? key === "new" ? null : key : previous.lastTaskId;
  const oldTask = key ? getTaskWorkspace(previous, key) : undefined;
  if (lastLocation === previous.lastLocation && lastTaskId === previous.lastTaskId && (!task || JSON.stringify(task) === JSON.stringify(oldTask))) return;
  const next = { ...previous, lastLocation, lastTaskId, tasks: key && task ? { ...previous.tasks, [key]: task } : previous.tasks };
  const metadataOnly = oldTask && task && lastLocation === previous.lastLocation
    && JSON.stringify({ ...oldTask, scroll: {}, sourceModes: {} }) === JSON.stringify({ ...task, scroll: {}, sourceModes: {} });
  projectCache.set(id, next);
  writerFor(id).queue(next, metadataOnly ? 250 : 0);
}

interface Props {
  projectId: string | null;
  taskKey: string;
  location: string;
  pane: Pane | undefined;
  isTask: boolean;
  demoOverview: boolean;
  state: RightPaneSessionState;
  apply: (state: RightPaneSessionState, saved: TaskWorkspace | undefined, restored: boolean) => void;
  getScroll: () => TaskWorkspace["scroll"];
  sourceModes: TaskWorkspace["sourceModes"];
  revision: number;
}

interface Committed {
  projectId: string;
  taskKey: string;
  scope: string;
  location: string;
  pane: Pane | undefined;
  state: RightPaneSessionState;
}

function snapshot(props: Pick<Props, "state" | "pane" | "getScroll" | "sourceModes">, previous: TaskWorkspace | undefined, projectId: string, taskKey: string): TaskWorkspace {
  const keys = new Set(props.state.fileTabs.map((tab) => fileScrollKey(projectId, taskKey === "new" ? null : taskKey, tab)));
  const scroll = Object.fromEntries(Object.entries(props.getScroll()).filter(([key]) => keys.has(key)));
  const sourceModes = Object.fromEntries(Object.entries(props.sourceModes).filter(([key]) => keys.has(key)));
  const task = rememberWorkspace(props.state, scroll, sourceModes);
  const active = props.pane ?? previous?.active;
  const activeKey = active ? rightTabKey(paneTab(active)) : null;
  task.active = task.tabs.find((tab) => rightTabKey(paneTab(tab)) === activeKey) ?? null;
  return task;
}

export function useProjectWorkspace(props: Props): {
  ready: boolean;
  loaded: boolean;
  error: string | null;
  retry: () => void;
  capture: () => void;
  workspace: MutableRefObject<ProjectWorkspace>;
} {
  const { projectId, taskKey, location, pane, isTask, demoOverview, state, apply, getScroll, sourceModes, revision } = props;
  const workspace = useRef<ProjectWorkspace>(emptyProjectWorkspace());
  const saveError = useSyncExternalStore(subscribeErrors, () => projectId ? saveErrors.get(projectId) ?? null : null);
  const [readError, setReadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [loadedProject, setLoadedProject] = useState<string | null>(null);
  const [renderedScope, setRenderedScope] = useState<string | null>(null);
  const appliedScope = useRef<string | null>(null);
  const appliedPane = useRef<string | null>(null);
  const committed = useRef<Committed | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const scope = JSON.stringify([projectId, taskKey, isTask]);
  const paneKey = JSON.stringify(pane ?? null);

  const capture = useCallback(() => {
    const previous = committed.current;
    if (!previous) return;
    const task = snapshot({ ...latest.current, state: previous.state, pane: previous.pane }, getTaskWorkspace(projectCache.get(previous.projectId), previous.taskKey), previous.projectId, previous.taskKey);
    save(previous.projectId, previous.location, previous.taskKey, task);
  }, []);

  useEffect(() => {
    let live = true;
    const visit = epoch;
    setReadError(null);
    setLoadedProject(null);
    if (!projectId) return;
    if (projectCache.has(projectId)) setLoadedProject(projectId);
    else void getProjectUiState(projectId).then((saved) => {
      if (!live || visit !== epoch) return;
      projectCache.set(projectId, saved ?? emptyProjectWorkspace());
      setLoadedProject(projectId);
    }).catch((error: unknown) => {
      if (live && visit === epoch) setReadError(error instanceof Error ? error.message : String(error));
    });
    return () => { live = false; };
  }, [projectId, attempt]);

  useLayoutEffect(() => {
    if (committed.current && committed.current.scope !== scope) {
      const previousProject = committed.current.projectId;
      capture();
      if (newTaskPromotions.get(previousProject) === taskKey) newTaskPromotions.delete(previousProject);
      committed.current = null;
    }
    if (!projectId || loadedProject !== projectId) return;
    const document = projectCache.get(projectId);
    if (!document) return;
    workspace.current = document;
    if (appliedScope.current !== scope) {
      appliedScope.current = scope;
      appliedPane.current = paneKey;
      if (isTask) {
        const saved = getTaskWorkspace(document, taskKey) ?? (isDemoProjectId(projectId) ? defaultTaskWorkspace(taskKey, demoOverview) : undefined);
        apply(restoreWorkspace(saved, pane), saved, true);
      }
      setRenderedScope(scope);
      return;
    }
    if (renderedScope !== scope) return;
    let resolvedState = state;
    if (appliedPane.current !== paneKey) {
      appliedPane.current = paneKey;
      if (isTask && pane) {
        resolvedState = applyPane(state, pane);
        apply(resolvedState, undefined, false);
      }
    }
    if (isTask) {
      save(projectId, location, taskKey, snapshot({ state: resolvedState, pane, getScroll, sourceModes }, getTaskWorkspace(document, taskKey), projectId, taskKey));
      committed.current = { projectId, taskKey, scope, location, pane, state: resolvedState };
    } else save(projectId, location);
    workspace.current = projectCache.get(projectId) ?? document;
  }, [projectId, taskKey, location, pane, paneKey, isTask, demoOverview, state, apply, getScroll, sourceModes, revision, loadedProject, scope, renderedScope, capture]);

  useEffect(() => {
    const visit = epoch;
    const flush = (unloading: boolean) => {
      if (visit !== epoch) return;
      capture();
      for (const writer of writers.values()) void writer.flush(unloading);
    };
    const onPageHide = () => flush(true);
    window.addEventListener("pagehide", onPageHide);
    return () => { window.removeEventListener("pagehide", onPageHide); flush(false); };
  }, [capture]);

  const retry = useCallback(() => {
    if (!projectId) return;
    if (readError) setAttempt((value) => value + 1);
    else void writers.get(projectId)?.retry();
  }, [projectId, readError]);

  return { ready: projectId === null || (loadedProject === projectId && renderedScope === scope), loaded: projectId === null || loadedProject === projectId, error: readError ?? saveError, retry, capture, workspace };
}
