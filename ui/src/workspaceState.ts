export type Pane =
  | { kind: "home"; view: "experiments" | "files" | "artifacts" }
  | { kind: "experiment"; experimentId: string; view: "overview" | "terminal"; runId?: string }
  | { kind: "file"; path: string; source?: "repo" | "artifacts" | "abs"; sessionId?: string; ref?: string; line?: number; branchLabel?: string }
  | { kind: "code"; experimentId: string; branch: string; view: "files" | "changes" }
  | { kind: "plan"; sessionId: string; promptId: string }
  | { kind: "subagent"; sessionId: string; spawnPartId: string };

export interface TaskWorkspace {
  tabs: Pane[];
  active: Pane | null;
  previewKey: string | null;
  history: string[];
  expanded: Record<string, string[]>;
  scroll: Record<string, { top: number; left: number }>;
  sourceModes: Record<string, boolean>;
  filesView: "files" | "changes";
  scope: "agent" | "project";
  panelMax: boolean;
}

export interface ProjectWorkspace {
  version: 1;
  lastTaskId: string | null;
  lastLocation: string | null;
  tasks: Record<string, TaskWorkspace>;
}

export function getTaskWorkspace(workspace: ProjectWorkspace | null | undefined, key: string): TaskWorkspace | undefined {
  return workspace && Object.hasOwn(workspace.tasks, key) ? workspace.tasks[key] : undefined;
}

export interface GlobalWorkspace {
  lastLocation: string | null;
  railOpen: boolean;
  panelWidth: number;
  experimentsView: "tree" | "table";
}

export const settingsTabs = ["settings", "harnesses", "projects", "compute", "instances", "environment", "git", "storage"] as const;
export type SettingsSection = typeof settingsTabs[number];
export const isSettingsSection = (value: unknown): value is SettingsSection =>
  settingsTabs.some((tab) => tab === value);

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const nonempty = (value: unknown): value is string => typeof value === "string" && value.length > 0;
const optionalString = (value: unknown) => value == null || nonempty(value);

export function parsePane(value: unknown): Pane | undefined {
  if (!isRecord(value)) return;
  const only = (...fields: string[]) => Object.keys(value).every((field) => fields.includes(field));
  switch (value.kind) {
    case "home":
      if (only("kind", "view") && (value.view === "experiments" || value.view === "files" || value.view === "artifacts"))
        return { kind: "home", view: value.view };
      break;
    case "experiment":
      if (only("kind", "experimentId", "view", "runId") && nonempty(value.experimentId) && (value.view === "overview" || value.view === "terminal") && optionalString(value.runId))
        return { kind: "experiment", experimentId: value.experimentId, view: value.view, ...(nonempty(value.runId) ? { runId: value.runId } : {}) };
      break;
    case "file":
      if (only("kind", "path", "source", "sessionId", "ref", "line", "branchLabel") && nonempty(value.path) && optionalString(value.sessionId) && optionalString(value.ref) && optionalString(value.branchLabel)
        && (value.source == null || value.source === "repo" || value.source === "artifacts" || value.source === "abs")
        && (value.line == null || (typeof value.line === "number" && Number.isSafeInteger(value.line) && value.line > 0)))
        return { kind: "file", path: value.path, ...(value.source ? { source: value.source } : {}),
          ...(nonempty(value.sessionId) ? { sessionId: value.sessionId } : {}), ...(nonempty(value.ref) ? { ref: value.ref } : {}),
          ...(typeof value.line === "number" ? { line: value.line } : {}), ...(nonempty(value.branchLabel) ? { branchLabel: value.branchLabel } : {}) };
      break;
    case "code":
      if (only("kind", "experimentId", "branch", "view") && nonempty(value.experimentId) && nonempty(value.branch) && (value.view === "files" || value.view === "changes"))
        return { kind: "code", experimentId: value.experimentId, branch: value.branch, view: value.view };
      break;
    case "plan":
      if (only("kind", "sessionId", "promptId") && nonempty(value.sessionId) && nonempty(value.promptId)) return { kind: "plan", sessionId: value.sessionId, promptId: value.promptId };
      break;
    case "subagent":
      if (only("kind", "sessionId", "spawnPartId") && nonempty(value.sessionId) && nonempty(value.spawnPartId)) return { kind: "subagent", sessionId: value.sessionId, spawnPartId: value.spawnPartId };
  }
}

export interface Destination {
  kind: "home" | "resume" | "task" | "skills" | "settings";
  projectId?: string;
  sessionId?: string;
  section?: SettingsSection;
}

export function parseDestination(pathname: string): Destination | null {
  if (pathname === "/projects") return { kind: "home" };
  const parts = pathname.split("/");
  if (parts[0] !== "" || parts[1] !== "projects" || !parts[2]) return null;
  let projectId: string;
  let leaf: string;
  try { projectId = decodeURIComponent(parts[2]); leaf = decodeURIComponent(parts[4] ?? ""); } catch { return null; }
  const validId = (id: string) => id.length > 0 && id !== "." && id !== ".." && !/[\\/?#\u0000-\u001f\u007f-\u009f]/.test(id);
  if (!validId(projectId)) return null;
  if (parts.length === 3 || (parts.length === 4 && parts[3] === "")) return { kind: "resume", projectId };
  if (parts.length === 4 && parts[3] === "skills") return { kind: "skills", projectId };
  if (parts.length === 5 && parts[3] === "tasks" && validId(leaf)) return { kind: "task", projectId, ...(leaf === "new" ? {} : { sessionId: leaf }) };
  if (parts.length === 5 && parts[3] === "settings" && isSettingsSection(leaf)) return { kind: "settings", projectId, section: leaf };
  return null;
}

export function safeLocation(value: unknown): string | null {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//") || /[\\#\u0000-\u001f\u007f-\u009f]/.test(value)) return null;
  const delimiter = value.indexOf("?");
  const pathname = delimiter === -1 ? value : value.slice(0, delimiter);
  const query = delimiter === -1 ? "" : value.slice(delimiter + 1);
  const destination = parseDestination(pathname);
  if (!destination || destination.kind === "resume") return null;
  const search = new URLSearchParams(query);
  if ([...search.keys()].some((key) => key !== "pane") || search.getAll("pane").length > 1) return null;
  if (search.has("pane")) {
    try { if (!parsePane(JSON.parse(search.get("pane") ?? ""))) return null; } catch { return null; }
  }
  return value;
}

export function taskLocation(projectId: string, sessionId: string | null, pane?: Pane | null): string {
  const path = `/projects/${encodeURIComponent(projectId)}/tasks/${sessionId ? encodeURIComponent(sessionId) : "new"}`;
  return pane ? `${path}?${new URLSearchParams({ pane: JSON.stringify(pane) })}` : path;
}

export const emptyProjectWorkspace = (): ProjectWorkspace => ({ version: 1, lastTaskId: null, lastLocation: null, tasks: {} });

// ponytail: one serialized writer per mounted workspace; add multi-window coordination only if needed.
export function createWorkspaceWriter<T>(save: (value: T, unloading: boolean) => Promise<unknown>, onError: (error: unknown) => void) {
  let pending: T | undefined;
  let failed: T | undefined;
  let running = false;
  let unloadPending = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  async function flush(unloading = false): Promise<void> {
    clearTimeout(timer);
    unloadPending ||= unloading;
    if (running || pending === undefined) return;
    running = true;
    const value = pending;
    pending = undefined;
    const keepalive = unloadPending;
    unloadPending = false;
    try { await save(value, keepalive); failed = undefined; }
    catch (error) { failed = value; onError(error); }
    finally { running = false; if (pending !== undefined) void flush(); }
  }
  return {
    queue(value: T, delay = 0) { pending = value; clearTimeout(timer); timer = setTimeout(() => void flush(), delay); },
    flush,
    retry() { if (!running && pending === undefined) pending = failed; return flush(); },
  };
}
