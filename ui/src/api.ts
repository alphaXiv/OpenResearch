// Typed client for the orx up local HTTP API (/api/*). All wire JSON is camelCase.

import { m } from "./paraglide/messages.js";
import { getLocale } from "./paraglide/runtime.js";
import { fmtNumber } from "./i18n";

export { fmtNumber } from "./i18n";

export const DEMO_PROJECT_ID = "demo_nanochat_v1";
// Bundled demo snapshots reserve this prefix so future demos inherit demo-only UI.
export const isDemoProjectId = (id: string) => id.startsWith("demo_");
export const DEMO_MAIN_SESSION_ID = "chat_demo_nanochat_v1";
export const DEMO_FIGURE_SESSION_ID = "chat_demo_nanochat_figures_v1";
export const DEMO_LITERATURE_SESSION_ID = "chat_demo_nanochat_literature_v1";
export const DEMO_OVERVIEW_ARTIFACT = "cpu-apple-silicon-pipeline-results.md";
export const DEMO_RUN_EXPERIMENT_PROMPT =
  "Run the Muon matrix LR 2× probe experiment. When it finishes, compare its step-100 and step-200 val_bpb against the baseline and tell me whether doubling the matrix learning rate helps early training.";

export interface Project {
  id: string;
  name: string;
  slug: string;
  githubOwner: string;
  githubRepo: string;
  baselineBranch: string;
  repoPath: string;
  path: string;
  githubEnabled: boolean;
  githubUrl?: string | null;
  /** Absolute path of the project's artifacts directory, non-canonical to
   *  match paths agents inline into chat. */
  artifactsDir: string;
  /** Compatibility alias returned by older/newer mixed local clients. */
  filesDir?: string;
  runCommand?: string | null;
  /** arXiv id the project starts from (versionless). */
  paperId?: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface Experiment {
  id: string;
  projectId: string;
  parentExperimentId?: string | null;
  slug: string;
  branchName: string;
  title?: string | null;
  description?: string | null;
  runCommand: string;
  agentStatus: string;
  createdAt: number;
  updatedAt: number;
  /** Chat session that created this experiment; null for dashboard/legacy rows. */
  chatSessionId?: string | null;
}

export type RunStatus = "starting" | "running" | "done" | "failed" | "cancelled";
export type RunDisplayStatus = RunStatus | "cancelling";

export interface Run {
  id: string;
  experimentId: string;
  projectId: string;
  status: RunStatus;
  backend?: Record<string, unknown> | null;
  command?: string | null;
  commitSha?: string | null;
  resultMarkdown?: string | null;
  createdAt: number;
  updatedAt: number;
  endedAt?: number | null;
  exitCode?: number | null;
  cancelRequested: boolean;
}

export function runDisplayStatus(run: Pick<Run, "status" | "cancelRequested">): RunDisplayStatus {
  const live = run.status === "running" || run.status === "starting";
  return live && run.cancelRequested ? "cancelling" : run.status;
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    let message = text;
    try {
      const parsed = JSON.parse(text) as { error?: string };
      if (parsed.error) message = parsed.error;
    } catch {
      // non-JSON body — show it raw
    }
    throw new Error(message || `HTTP ${res.status}`);
  }
  return (await res.json()) as T;
}

const get = <T>(url: string) => fetch(url).then((r) => json<T>(r));
const post = <T>(url: string, body?: unknown) =>
  fetch(url, {
    method: "POST",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then((r) => json<T>(r));
const patch = <T>(url: string, body: unknown) =>
  fetch(url, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  }).then((r) => json<T>(r));
const put = <T>(url: string, body: unknown) =>
  fetch(url, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  }).then((r) => json<T>(r));

export const listProjects = () =>
  get<{ projects: Project[] }>("/api/projects").then((r) => r.projects);

export interface ProjectActivity {
  projectId: string;
  activeAgents: number;
  /** Lifetime sessions for this project, including archived agents. */
  totalAgents: number;
  runningExperiments: number;
  totalExperiments: number;
  lastMessageAt: number | null;
}

export const listProjectActivity = () =>
  get<{ activity: ProjectActivity[] }>("/api/projects/activity").then((r) => r.activity);

export interface OnboardingSelection {
  harness: HarnessId;
  model: string | null;
  serviceTier?: string | null;
  permissionMode: string | null;
  reasoningLevel: string | null;
}

export type AgentSelection = OnboardingSelection;

export interface UiState {
  onboardingCompleted: boolean;
  tourCompleted: boolean;
  preferredAgent: AgentSelection | null;
}

export const getUiState = () => get<UiState>("/api/settings/ui-state");

export const updateUiState = (body: {
  tourCompleted?: boolean;
  preferredAgent?: AgentSelection;
}) => post<UiState>("/api/settings/ui-state", body);

export const completeOnboarding = (selection: OnboardingSelection, profile: Profile) =>
  post<{ project: Project; selection: OnboardingSelection }>(
    "/api/onboarding/complete",
    { ...selection, ...profile },
  );

export interface ProjectPathStatus {
  gitVersion: string | null;
  resolvedPath: string | null;
  exists: boolean | null;
  directory: boolean | null;
  empty: boolean | null;
  initialized: boolean | null;
  gitState?: "notRepository" | "unborn" | "ready" | "detached" | "invalid" | null;
  githubOwner?: string | null;
  githubRepo?: string | null;
}

export const getProjectPathStatus = (path = "") => {
  const query = path ? `?path=${encodeURIComponent(path)}` : "";
  return get<ProjectPathStatus>(`/api/project-path/status${query}`);
};

export const pickProjectFolder = () =>
  post<{ path: string | null }>("/api/project-path/pick").then((result) => result.path);

export interface NewProject {
  name: string;
  path: string;
  runCommand?: string;
  paperId?: string;
  cloneUrl?: string;
  createFolder?: boolean;
  requireNewFolder?: boolean;
  initializeGit?: boolean;
  githubSyncEnabled?: boolean;
  /** UI locale for the starter prompts the server warms up on creation. */
  locale?: string;
}

export interface CreateProjectResult {
  project: Project;
  githubPublicationError: string | null;
}

export const createProject = (body: NewProject) =>
  post<CreateProjectResult>("/api/projects", body);

export interface PaperHit {
  paperId: string;
  title: string;
  snippet?: string | null;
}

export interface ResolvedPaper {
  paperId: string;
  title?: string | null;
  repoUrl?: string | null;
  repoStars?: number | null;
}

export const searchPapers = (q: string) =>
  get<{ papers: PaperHit[] }>(`/api/papers/search?q=${encodeURIComponent(q)}`).then(
    (r) => r.papers,
  );

/** The signed-in GitHub login, for naming the account a new repo lands on.
 * `login` is null when there's no usable token. */
export const githubAccount = () => get<{ login: string | null }>("/api/github/account");

export const githubProjectRepoPreview = (name: string) =>
  get<{ repo: string }>(`/api/github/project-repo-preview?name=${encodeURIComponent(name)}`);

/** Whether the stored credentials are explicitly confirmed to push to a repo. */
export const repoAccess = (owner: string, repo: string) =>
  get<{ canPush: boolean }>(
    `/api/github/repo-access?owner=${encodeURIComponent(owner)}&repo=${encodeURIComponent(repo)}`,
  );

/** Resolve an arXiv id / URL to title + linked GitHub repo. May take a few
 * seconds for papers alphaXiv hasn't indexed yet (it scrapes arXiv on a miss). */
export const resolvePaper = (id: string) =>
  get<{ paper: ResolvedPaper }>(`/api/papers/resolve?id=${encodeURIComponent(id)}`).then(
    (r) => r.paper,
  );

export const updateProject = (projectId: string, body: { runCommand?: string; name?: string }) =>
  patch<{ project: Project }>(`/api/projects/${projectId}`, body).then((r) => r.project);

/** One suggested opening message for the empty chat, written by a model that
 *  read the project. */
export interface StarterPrompt {
  title: string;
  prompt: string;
}

export interface ProjectStarterPrompts {
  /** Null when the project already has experiments or no harness could answer. */
  prompts: StarterPrompt[] | null;
}

/** Start generating starter prompts for a project that is about to be created,
 *  so they are cached by the time its chat opens. Fire-and-forget. */
export const prewarmStarterPrompts = (body: {
  name: string;
  paperId?: string;
  path?: string;
  locale: string;
}) => post<{ ok: boolean }>("/api/projects/starter-prompts/prewarm", body);

/** Slow on a cache miss (one headless model call over a brief of the project). */
export const getProjectStarterPrompts = (
  projectId: string,
  harness: HarnessId,
  model: string | null,
  locale: string,
) =>
  get<ProjectStarterPrompts>(
    `/api/projects/${projectId}/starter-prompts?${new URLSearchParams({ harness, ...(model ? { model } : {}), locale })}`,
  );

/** Record a visit so the backend can persist project-level UI recency. */
export const openProject = (projectId: string) =>
  post<{ project: Project }>(`/api/projects/${projectId}/open`).then((r) => r.project);

export const deleteProject = (projectId: string) =>
  fetch(`/api/projects/${projectId}`, { method: "DELETE" }).then(async (r) => {
    if (!r.ok) {
      const body = await r.json().catch(() => null);
      throw new Error(body?.error ?? `delete failed (${r.status})`);
    }
  });

export const listExperiments = (projectId: string) =>
  get<{ experiments: Experiment[] }>(`/api/projects/${projectId}/experiments`).then(
    (r) => r.experiments,
  );

export const listRuns = (projectId: string) =>
  get<{ runs: Run[] }>(`/api/projects/${projectId}/runs`).then((r) => r.runs);

/** A run viewed as compute: every run across all projects, tagged with the
 *  name of the project that launched it. `projectName` is enriched only on the
 *  /api/instances snapshot — it is absent from the `run.updated` SSE payload. */
export interface Instance extends Run {
  projectName?: string;
}

export const listInstances = () =>
  get<{ instances: Instance[] }>("/api/instances").then((r) => r.instances);

export const cancelRun = (runId: string) =>
  post<{ ok: boolean }>(`/api/runs/${runId}/cancel`).then(() => undefined);

export interface LogChunk {
  dataBase64: string;
  nextOffset: number;
  eof: boolean;
}

export const fetchLog = (runId: string, offset: number) =>
  get<LogChunk>(`/api/runs/${runId}/log?offset=${offset}`);

export interface DiffPayload {
  diff: string;
  truncated: boolean;
  bytesRead: number;
  byteLimit: number;
}

export const getRunDiff = (runId: string) => get<DiffPayload>(`/api/runs/${runId}/diff`);

export const getExperimentDiff = (experimentId: string) =>
  get<DiffPayload>(`/api/experiments/${experimentId}/diff`);

/** Which source answered a checkout read: a session's live worktree, the hub
 * clone (also the worktree-pruned fallback), or a branch's committed tree. */
export type CheckoutRoot = "worktree" | "clone" | "branch";

/** Source selector for checkout reads: `ref` picks a branch's committed
 * state; `sessionId` picks the session's live worktree; neither picks the hub
 * clone. Don't send both — the file endpoint ignores `sessionId` under `ref`,
 * but code-tree rejects the combination outright. */
export interface CheckoutRef {
  sessionId?: string;
  ref?: string;
}

const checkoutQuery = (opts: CheckoutRef, params: URLSearchParams = new URLSearchParams()) => {
  if (opts.sessionId) params.set("sessionId", opts.sessionId);
  if (opts.ref) params.set("ref", opts.ref);
  return params;
};

export interface ProjectFile {
  path: string;
  content: string;
  truncated: boolean;
  binary: boolean;
  notFound: boolean;
  root: CheckoutRoot;
  presentation: FilePresentation;
}

/** One file from the project — a branch's committed copy when `ref` is given,
 * else a chat session's worktree, else the hub clone — capped server-side
 * (~512 KB). */
export const getProjectFile = (projectId: string, path: string, opts: CheckoutRef = {}) =>
  get<ProjectFile>(
    `/api/projects/${projectId}/file?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
  );

/** Byte-exact checkout file for browser-native media rendering or download. */
export const projectFileUrl = (projectId: string, path: string, opts: CheckoutRef = {}) =>
  `/api/projects/${projectId}/file/raw?${checkoutQuery(opts, new URLSearchParams({ path }))}`;

/** A file read by absolute path, outside the project's checkout and artifacts
 * (e.g. `/Users/me/.ssh/config`). Same capped/decoded body as `ProjectFile`;
 * the wire `root` is always `"abs"`, so it's dropped here rather than widening
 * `CheckoutRoot` — nothing reads it, and it isn't part of any git tree. */
export type AbsoluteFile = Omit<ProjectFile, "root">;

/** One file by absolute path — the escape hatch for a file an agent references
 * that lives outside the checkout and artifacts. Server-side capped (~512 KB);
 * loopback-only, so it reads whatever the user running `orx up` can read. */
export const getAbsoluteFile = (path: string) =>
  get<AbsoluteFile>(`/api/files/abs?path=${encodeURIComponent(path)}`);

/** Byte-exact absolute-path file for browser-native media rendering or download. */
export const absoluteFileUrl = (path: string) =>
  `/api/files/abs/raw?path=${encodeURIComponent(path)}`;

/** Overwrite a text file in the project's live checkout (worktree when
 * `sessionId` is given, else the hub clone). Committed branch trees are
 * read-only, so pass no `ref`. */
export const saveProjectFile = (
  projectId: string,
  path: string,
  content: string,
  opts: { sessionId?: string } = {},
) =>
  put<{ ok: boolean; root: CheckoutRoot; bytesWritten: number }>(
    `/api/projects/${projectId}/file`,
    { path, content, sessionId: opts.sessionId },
  );

/** Open a checkout file on the machine running `orx up`, in the OS default app
 * for its type (the user's editor for source files). */
export const openFileInEditor = (
  projectId: string,
  path: string,
  opts: { sessionId?: string } = {},
) =>
  post<{ ok: boolean }>(`/api/projects/${projectId}/file/open`, {
    path,
    sessionId: opts.sessionId,
  });

export interface LatexEngine {
  /** The engine that will run, or null when the machine has none. */
  engine: string | null;
  /** Install guidance, present only when `engine` is null. */
  hint: string | null;
  /** A paste-ready install command, where this platform has one. */
  installCommand: string | null;
}

/** Whether the machine running `orx up` can compile LaTeX. */
export const getLatexEngine = () => get<LatexEngine>("/api/latex/engine");

export interface LatexCompileResult {
  ok: boolean;
  /** Checkout-relative path of the produced PDF, null when the run failed. */
  pdfPath: string | null;
  /** What ran: `latexmk (xelatex)`, `tectonic`, `pdflatex`. */
  engine: string;
  /** Set when the toolchain could not honour the document's `% !TeX program`. */
  note: string | null;
  /** The engine reported errors. With `ok` true the PDF exists anyway — TeX
   * recovers from most of them — but the log is worth showing. */
  hadErrors: boolean;
  /** Tail of the engine's output — the only useful thing on a failure. */
  log: string;
}

/** Compile a checkout `.tex` file to a PDF beside it, using whichever LaTeX
 * engine is installed on the machine running `orx up`. Rejects with a message
 * naming the install options when none is. */
export const compileLatex = (
  projectId: string,
  path: string,
  opts: { sessionId?: string } = {},
) =>
  post<LatexCompileResult>(`/api/projects/${projectId}/file/latex`, {
    path,
    sessionId: opts.sessionId,
  });

/** The Overleaf project a `.tex` pushes to. */
export interface OverleafLink {
  projectId: string;
  /** The project on overleaf.com, for opening it. */
  url: string;
}

export interface OverleafState {
  /** A Git authentication token is stored on this machine. */
  hasToken: boolean;
  /** Null until this paper is pointed at a project. */
  link: OverleafLink | null;
}

/** Whether a Git authentication token is stored — the machine-wide half of the
 * Overleaf state, for Settings. */
export const getOverleafSettings = () => get<{ hasToken: boolean }>("/api/overleaf/settings");

/** Store an Overleaf Git authentication token. Not validated here: only the Git
 * bridge can judge a token, and it needs a project to judge it against, so a
 * bad token surfaces from `linkOverleaf`. */
export const saveOverleafToken = (token: string) =>
  post<{ hasToken: boolean }>("/api/overleaf/token", { token });

export const deleteOverleafToken = () =>
  fetch("/api/overleaf/token", { method: "DELETE" }).then((r) => json<{ hasToken: boolean }>(r));

export const getOverleafState = (
  projectId: string,
  path: string,
  opts: { sessionId?: string } = {},
) => get<OverleafState>(`/api/projects/${projectId}/file/overleaf?${checkoutQuery(opts, new URLSearchParams({ path }))}`);

/** Point this `.tex` at an Overleaf project. The server proves the account can
 * reach it before storing the link, so this is where a plan without Git
 * integration — or a bad token — is reported. */
export const linkOverleaf = (
  projectId: string,
  path: string,
  opts: { project: string; sessionId?: string },
) =>
  post<OverleafState>(`/api/projects/${projectId}/file/overleaf`, {
    path,
    project: opts.project,
    sessionId: opts.sessionId,
  });

export const unlinkOverleaf = (projectId: string, path: string, opts: { sessionId?: string } = {}) =>
  fetch(`/api/projects/${projectId}/file/overleaf?${checkoutQuery(opts, new URLSearchParams({ path }))}`, {
    method: "DELETE",
  }).then((r) => json<OverleafState>(r));

/** How the user settled a file both sides changed, keyed by checkout-relative path. */
export type OverleafResolution = "keep-local" | "take-overleaf";

export interface OverleafSyncResult {
  ok: boolean;
  /** Files Overleaf changed alone, now written into the checkout. */
  pulled: string[];
  /** Files we changed alone, now committed to Overleaf. */
  pushed: string[];
  /** Files both sides changed since the last sync. Left untouched on both
   * sides until the user says which copy to keep. */
  conflicts: string[];
  /** Main-document mismatch, or files left behind. */
  note: string | null;
}

/** Bring the paper and the linked Overleaf project into step, both ways. */
export const syncOverleaf = (
  projectId: string,
  path: string,
  opts: { sessionId?: string; resolve?: Record<string, OverleafResolution> } = {},
) =>
  post<OverleafSyncResult>(`/api/projects/${projectId}/file/overleaf/sync`, {
    path,
    sessionId: opts.sessionId,
    resolve: opts.resolve,
  });

/** Whether Overleaf has moved since the last sync. One request and no transfer,
 * so a linked paper can be watched while its tab is open. */
export const getOverleafStatus = (
  projectId: string,
  path: string,
  opts: { sessionId?: string } = {},
) =>
  get<{ remoteChanged: boolean }>(
    `/api/projects/${projectId}/file/overleaf/status?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
  );

/** Page that posts the paper to Overleaf as a new project — the path for an
 * account whose plan has no Git integration. Opened in a tab, not fetched. */
export const overleafUploadUrl = (
  projectId: string,
  path: string,
  opts: { sessionId?: string } = {},
) => `/api/projects/${projectId}/file/overleaf/upload?${checkoutQuery(opts, new URLSearchParams({ path }))}`;

export interface CodeTree {
  root: CheckoutRoot;
  /** The listed branch (`ref` mode), else the checked-out branch, else null
   * (detached HEAD). */
  branch: string | null;
  /** Repo-relative file paths (gitignored trees excluded), sorted. */
  entries: string[];
  /** True when the listing hit the server-side cap (20,000 entries). */
  truncated: boolean;
}

/** Flat file listing of the project — a branch's committed tree when `ref` is
 * given, else a chat session's live worktree when `sessionId` is given, else
 * the hub clone's checkout — plus the branch name. `ref` and `sessionId` are
 * mutually exclusive (the server rejects both). */
export const getCodeTree = (projectId: string, opts: CheckoutRef = {}) => {
  const qs = checkoutQuery(opts).toString();
  return get<CodeTree>(`/api/projects/${projectId}/code-tree${qs ? `?${qs}` : ""}`);
};

/** How a file in a session's worktree differs from the diff base. Lowercase to
 * match the server's serialization and the single-letter badges the UI draws. */
export type ChangedStatus = "added" | "modified" | "deleted" | "renamed" | "untracked";

export interface ChangedFile {
  path: string;
  status: ChangedStatus;
  /** Pre-rename path — present only for `renamed` entries. */
  oldPath?: string;
}

/** Live view of a chat session's private worktree. `exists: false` when the
 * agent hasn't started yet (the worktree is created lazily on the first turn)
 * or was pruned — the remaining fields are then absent. `files` is the
 * complete change list even when `diff` truncates (they come from separate git
 * passes); `diff` is the working tree against the baseline merge-base, with
 * untracked files rendered as new-file diffs. */
export interface SessionWorktree {
  exists: boolean;
  /** Checked-out branch, or null when detached at the baseline tip. */
  branch?: string | null;
  baselineBranch?: string;
  baseSha?: string;
  files?: ChangedFile[];
  diff?: DiffPayload;
}

export const getSessionWorktree = (sessionId: string) =>
  get<SessionWorktree>(`/api/chat/sessions/${sessionId}/worktree`);

/** A GitHub `tree` URL for a branch. Branch names contain `/` (`orx/<slug>`),
 * so encode each path segment — never the whole string, which would escape the
 * slashes. Unpushed branches 404 on GitHub, which is acceptable. */
export const githubBranchUrl = (owner: string, repo: string, branch: string) =>
  `https://github.com/${owner}/${repo}/tree/${branch
    .split("/")
    .map(encodeURIComponent)
    .join("/")}`;

export type HfTokenSource = "env" | "openresearchEnv" | "hfCache";

export interface HfSettings {
  configured: boolean;
  source: HfTokenSource | null;
  maskedToken: string | null;
  valid: boolean;
  username: string | null;
  jobsWrite: boolean | null;
}

export const getHfSettings = () => get<HfSettings>("/api/settings/hf");

export const saveHfToken = (token: string) => post<HfSettings>("/api/settings/hf", { token });

// --- updates ------------------------------------------------------------------

/** How orx was installed. Only `installer` and `app-bundle` update themselves. */
export type InstallChannel = "installer" | "app-bundle" | "cargo" | "homebrew" | "nix" | "unknown";

export interface UpdateStatus {
  current: string;
  /** Latest release this install can actually move to — the macOS app and the
   *  CLI read different manifests, and the app's can lag. */
  latest: string | null;
  channel: InstallChannel;
  /** Whether this install is one orx can replace at all. */
  selfUpdates: boolean;
  autoUpdate: boolean;
  /** `autoUpdate` is off because of the environment, not the user's setting. */
  envDisabled: boolean;
  updateAvailable: boolean;
  /** The newer version already on disk. Distinct from `latest`: a release can
   *  land between the install and the restart. */
  installedVersion: string | null;
  restartRequired: boolean;
}

export interface InstalledCli {
  link: string;
  /** The link's directory — what to add to PATH when `onPath` is false. */
  dir: string;
  target: string;
  onPath: boolean;
  alreadyCurrent: boolean;
}

export const getUpdateStatus = () => get<UpdateStatus>("/api/update");

export const applyUpdate = () => post<UpdateStatus>("/api/update/apply");

export const setAutoUpdate = (enabled: boolean) =>
  post<UpdateStatus>("/api/update/auto", { enabled });

export const installCli = (force = false) =>
  post<InstalledCli>("/api/update/install-cli", { force });

// --- settings: kubernetes -----------------------------------------------------

export interface K8sPreflight {
  kubectlFound: boolean;
  reachable: boolean;
  canCreateJobs: boolean;
  error?: string;
}

export interface K8sSettings {
  configured: boolean;
  contexts: string[];
  currentContext: string | null;
  context: string | null;
  namespace: string;
  preflight: K8sPreflight;
}

export const getK8sSettings = () => get<K8sSettings>("/api/settings/k8s");

export const saveK8sSettings = (body: { context?: string; namespace?: string }) =>
  post<K8sSettings>("/api/settings/k8s", body);

// --- settings: modal ----------------------------------------------------------

export type ModalTokenSource = "env" | "syncedEnv" | "modalToml";

export interface ModalSettings {
  /** The orx-managed venv exists on disk. */
  envProvisioned: boolean;
  /** `import modal` succeeds with the resolved interpreter. */
  modalImportable: boolean;
  tokenConfigured: boolean;
  tokenSource: ModalTokenSource | null;
  /** modalImportable && tokenConfigured. */
  ready: boolean;
  error: string | null;
}

export const getModalSettings = () => get<ModalSettings>("/api/settings/modal");

/** Build the orx-managed Modal env (first run downloads the SDK, ~30–60s). */
export const provisionModal = () => post<ModalSettings>("/api/settings/modal/provision");

// --- settings: env vars / git / harnesses ------------------------------------

export interface EnvVar {
  key: string;
  maskedValue: string;
  inProcessEnv: boolean;
}

export const getEnvVars = () =>
  get<{ vars: EnvVar[] }>("/api/settings/env").then((r) => r.vars);

export const setEnvVar = (key: string, value: string) =>
  post<{ vars: EnvVar[] }>("/api/settings/env", { key, value }).then((r) => r.vars);

export const deleteEnvVar = (key: string) =>
  fetch(`/api/settings/env/${encodeURIComponent(key)}`, { method: "DELETE" })
    .then((r) => json<{ vars: EnvVar[] }>(r))
    .then((r) => r.vars);

/** Where `source` says the resolved data dir came from. `env` means the
 * `$ORX_DATA_DIR` override forces it — the UI shows the field read-only. */
export type DataDirSource = "env" | "config" | "xdg" | "default";

export interface DataDirSettings {
  current: string;
  defaultPath: string;
  isDefault: boolean;
  source: DataDirSource;
}

export const getDataDir = () => get<DataDirSettings>("/api/settings/data-dir");

export interface DataDirValidation {
  ok: boolean;
  error?: string;
  treeBytes?: number;
  freeBytes?: number;
  sameFilesystem?: boolean;
}

export const validateDataDir = (path: string) =>
  post<DataDirValidation>("/api/settings/data-dir/validate", { path });

/** Set the path without moving (onboarding / already-populated target). */
export const setDataDir = (path: string) =>
  post<DataDirSettings>("/api/settings/data-dir", { path });

/** Kick off a relocate. Resolves once the move has *started* (HTTP 202); watch
 * `onDataDirMove` (events.ts) for `progress` / `done` / `error`. Throws on the
 * 409 in-flight guard with the server's message. */
export const moveDataDir = (path: string) =>
  post<{ started: boolean }>("/api/settings/data-dir/move", { path });

export interface SshHost {
  host: string;
  hostname?: string;
  user?: string;
  port?: string;
  identityFile?: string;
  /** Most recent preflight result, persisted across restarts. */
  lastTest?: SshPreflight;
}

export const getSshHosts = () =>
  get<{ hosts: SshHost[] }>("/api/settings/ssh").then((r) => r.hosts);

export interface SshConfigFile {
  path: string;
  content: string;
}

export const getSshConfig = () => get<SshConfigFile>("/api/settings/ssh/config");

export const saveSshConfig = (content: string, previousContent: string) =>
  put<{ ok: boolean }>("/api/settings/ssh/config", { content, previousContent });

export const getSshMasterStatus = (host: string) =>
  get<{ running: boolean }>(`/api/settings/ssh/master?host=${encodeURIComponent(host)}`);

export type RemoteSessionStatus =
  | "checking"
  | "needsInstall"
  | "needsUpdate"
  | "installing"
  | "updating"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "failed";

export interface RemoteInstallPaths {
  binary: string;
  database: string;
  cache: string;
}

export interface RemoteSessionInfo {
  id: string;
  host: string;
  user: string | null;
  status: RemoteSessionStatus;
  version: string | null;
  dashboardProtocol: number | null;
  gatewayUrl: string;
  error: string | null;
  installPaths: RemoteInstallPaths | null;
  uiPreferences: { theme: string | null; locale: string | null };
  canStartNewHost: boolean;
}

export type RuntimeInfo =
  | { kind: "local"; version: string }
  | { kind: "ssh"; version: string; session: RemoteSessionInfo };

export const getRuntime = () => get<RuntimeInfo>("/_orx/runtime");

export const listRemoteSessions = () =>
  get<{ sessions: RemoteSessionInfo[] }>("/api/remote/sessions").then((r) => r.sessions);

export const createRemoteSession = (
  host: string,
  uiPreferences: { theme: string | null; locale: string | null },
) => post<RemoteSessionInfo>("/api/remote/sessions", { host, uiPreferences });

export const reconnectRemoteSession = (id: string) =>
  post<RemoteSessionInfo>(`/api/remote/sessions/${encodeURIComponent(id)}/reconnect`);

export const disconnectRemoteSession = (id: string) =>
  post<RemoteSessionInfo>(`/api/remote/sessions/${encodeURIComponent(id)}/disconnect`);

export const installRemoteSession = (paths: RemoteInstallPaths) =>
  post<RemoteSessionInfo>("/_orx/install", paths);

export const reconnectCurrentRemote = () => post<RemoteSessionInfo>("/_orx/reconnect");

export const disconnectCurrentRemote = () => post<RemoteSessionInfo>("/_orx/disconnect");

export const startCurrentRemoteHost = () => post<RemoteSessionInfo>("/_orx/start-host");

export interface RemoteStopPreview {
  instanceId: string;
  activeTurnCount: number;
  queuedMessageCount: number;
  pendingPermissionCount: number;
  activeRunCount: number;
  attachmentCount: number;
}

export const getRemoteStopPreview = () => get<RemoteStopPreview>("/_orx/stop-host");

export const stopCurrentRemoteHost = (preview: RemoteStopPreview) =>
  post<{ accepted: boolean }>("/_orx/stop-host", {
    expectedInstanceId: preview.instanceId,
    expectedPreview: {
      activeTurnCount: preview.activeTurnCount,
      queuedMessageCount: preview.queuedMessageCount,
      pendingPermissionCount: preview.pendingPermissionCount,
      activeRunCount: preview.activeRunCount,
      attachmentCount: preview.attachmentCount,
    },
  });

export interface SshPreflight {
  reachable: boolean;
  toolsFound: boolean;
  missingTools?: string[];
  error: string | null;
  /** Unix millis. */
  testedAt: number;
}

// --- settings: slurm ----------------------------------------------------------

export interface SlurmSettings {
  /** Default login node (an ~/.ssh/config alias); null = must pass --host. */
  host: string | null;
  /** Cluster defaults; null = the cluster decides. */
  partition: string | null;
  account: string | null;
  timeLimit: string | null;
  /** Login-node candidates, from ~/.ssh/config (same source as SSH). */
  hosts: SshHost[];
}

export const getSlurmSettings = () => get<SlurmSettings>("/api/settings/slurm");

/** Empty string clears a field back to the cluster default. */
export const saveSlurmSettings = (body: {
  host?: string;
  partition?: string;
  account?: string;
  timeLimit?: string;
}) => post<SlurmSettings>("/api/settings/slurm", body);

export interface SlurmPreflight {
  reachable: boolean;
  slurmFound: boolean;
  toolsFound: boolean;
  partitions: string[];
  error: string | null;
}

// --- settings: ray ------------------------------------------------------------

export interface RaySettings {
  /** Saved Jobs / Dashboard URL; null = fall back to env / localhost. */
  address: string | null;
  /** Effective address after settings → env → default resolution. */
  resolvedAddress: string;
  /** settings | ASTROAI_RAY_JOBS_ADDRESS | RAY_DASHBOARD_URL | default */
  source: string;
}

export const getRaySettings = () => get<RaySettings>("/api/settings/ray");

/** Empty string clears the saved address (fall back to env / default). */
export const saveRaySettings = (body: { address?: string }) =>
  post<RaySettings>("/api/settings/ray", body);

export interface RayPreflight {
  reachable: boolean;
  address: string;
  rayVersion: string | null;
  error: string | null;
}

/** Live-test a Ray Jobs / Dashboard endpoint. */
export const rayPreflight = (address?: string) =>
  post<RayPreflight>("/api/settings/ray/preflight", { address: address ?? null });

// --- settings: compute targets (unified list + default) ------------------------

export type ComputeTargetId =
  | "local"
  | "tinker"
  | "hf"
  | "modal"
  | "k8s"
  | "ssh"
  | "slurm"
  | "ray"
  | "openresearch";

/** Cheap fs/env probe only — "worth trying", not "healthy". Deep health lives
 * in each backend's own settings endpoint, fetched when its row is expanded. */
export interface ComputeTargetSummary {
  id: ComputeTargetId;
  configured: boolean;
  /**
   * The readiness check couldn't run (offline, unreadable ~/.ssh), so
   * `configured` is a guess rather than an answer. Absent for backends whose
   * state is decidable locally.
   */
  unverified?: boolean;
  summary: string;
  enabled: boolean;
  disabledReason?: string | null;
}

export interface ComputeSettings {
  defaultBackend: ComputeTargetId | null;
  defaultFlavor: string | null;
  targets: ComputeTargetSummary[];
  configuredDefaultBackend?: ComputeTargetId | null;
  configuredDefaultFlavor?: string | null;
}

export const getComputeSettings = (projectId?: string) =>
  get<ComputeSettings>(
    `/api/settings/compute${projectId ? `?projectId=${encodeURIComponent(projectId)}` : ""}`,
  );

/** Set (or clear, with backend: null) the default compute target. Responds
 * with the full compute payload so the caller reconciles in one shot. */
export const setComputeDefault = (body: {
  backend: ComputeTargetId | null;
  flavor?: string | null;
  projectId?: string;
}) => post<ComputeSettings>("/api/settings/compute/default", body);

export interface LocalGpu {
  name: string;
  memMib: number | null;
}

/** What `--backend local` runs on: this machine's detected hardware. */
export interface LocalMachine {
  hostname: string;
  os: string;
  arch: string;
  /** CPU brand string on macOS (e.g. "Apple M2 Pro"). */
  chip: string | null;
  cpuCount: number;
  memBytes: number | null;
  gpus: LocalGpu[];
}

export const getLocalMachine = () => get<LocalMachine>("/api/settings/local");

export interface OpenResearchSettings {
  loggedIn: boolean;
  apiUrl: string | null;
  orgs: string[];
  /**
   * Whether a registered key's private half is on THIS machine — `matched` is
   * the only state that can actually reach a box. Optional: an older `orx`
   * binary serving a newer ui omits it.
   */
  sshKeyStatus?: "matched" | "no_local_match" | "none_registered" | "unknown";
  /** The `.pub` on this machine worth registering; null if there isn't one. */
  sshKeyPath?: string | null;
  error: string | null;
}

export const getOpenResearchSettings = () =>
  get<OpenResearchSettings>("/api/settings/openresearch");

/** One node of the artifacts tree: a file, or a directory with children. */
export interface ArtifactEntry {
  name: string;
  /** Directory-relative `/`-joined path — the id for read/delete endpoints. */
  path: string;
  isDir: boolean;
  /** 0 for directories. */
  size: number;
  modifiedAt: number;
  presentation?: FilePresentation;
  children?: ArtifactEntry[];
}

export type FilePresentation = "image" | "audio" | "video" | "pdf" | "text" | "unknown" | "download";

/** Listing of the project's on-disk artifacts directory. */
export interface ProjectArtifacts {
  dir: string;
  entries: ArtifactEntry[];
  truncated: boolean;
}

export const getArtifacts = (projectId: string) =>
  get<ProjectArtifacts>(`/api/projects/${projectId}/files`);

/** Delete a file or folder in the artifacts directory. */
export const deleteArtifact = (projectId: string, path: string) =>
  fetch(`/api/projects/${projectId}/files?path=${encodeURIComponent(path)}`, {
    method: "DELETE",
  }).then((r) => json<{ ok: boolean }>(r));

/** Raw artifact bytes served by the compatibility `/files` API. */
export const artifactUrl = (projectId: string, path: string) =>
  `/api/projects/${projectId}/files/file?path=${encodeURIComponent(path)}`;

export interface FileTextBody {
  content: string;
  binary: boolean;
  truncated: boolean;
}

export const FILE_PREVIEW_BYTES = 512_000;

/** Decode a raw response only when it is valid, NUL-free UTF-8. */
const decodeFileText = (bytes: ArrayBuffer, truncated: boolean): FileTextBody => {
  const view = new Uint8Array(bytes);
  if (view.includes(0)) return { content: "", binary: true, truncated };
  try {
    const decoder = new TextDecoder("utf-8", { fatal: true });
    const content = decoder.decode(view, { stream: truncated });
    return { content, binary: false, truncated };
  } catch {
    return { content: "", binary: true, truncated };
  }
};

/** Text-safe body of an artifact, or `null` when the file is missing. */
export const getArtifactFileText = (
  projectId: string,
  path: string,
): Promise<FileTextBody | null> => {
  return fetch(artifactUrl(projectId, path), {
    headers: { Range: `bytes=0-${FILE_PREVIEW_BYTES - 1}` },
  }).then((r) => {
    if (r.status === 404) return null;
    if (r.status === 416 && r.headers.get("content-range") === "bytes */0") {
      return { content: "", binary: false, truncated: false };
    }
    // Bare message — the viewer prefixes "Failed to load file:" itself.
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const total = Number(r.headers.get("content-range")?.split("/").pop());
    return r.arrayBuffer().then((bytes) => decodeFileText(bytes, Number.isFinite(total) && total > bytes.byteLength));
  });
};

const isFilePresentation = (value: string | null): value is FilePresentation =>
  value === "image" || value === "audio" || value === "video" || value === "pdf" ||
  value === "text" || value === "unknown" || value === "download";

export interface ArtifactFileMetadata {
  size: number;
  presentation: FilePresentation;
}

/** Lightweight existence/type probe used before an artifact preview. */
export const getArtifactFileMetadata = (
  projectId: string,
  path: string,
): Promise<ArtifactFileMetadata | null> =>
  fetch(artifactUrl(projectId, path), { method: "HEAD" }).then((r) => {
    if (r.status === 404) return null;
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const presentation = r.headers.get("x-openresearch-presentation");
    return {
      size: Number(r.headers.get("content-length")) || 0,
      presentation: isFilePresentation(presentation) ? presentation : "download",
    };
  });

export interface GitSettings {
  gitVersion: string | null;
  userName: string | null;
  userEmail: string | null;
  ghInstalled: boolean;
  githubAuthenticated: boolean;
}

export const getGitSettings = () => get<GitSettings>("/api/settings/git");

export const saveGitSettings = (body: { userName?: string; userEmail?: string }) =>
  post<GitSettings>("/api/settings/git", body);

/** A paper linked to the researcher profile during onboarding. */
export interface LinkedPaper {
  paperId: string;
  title: string | null;
}

/** The local researcher profile captured in onboarding (settings.json). */
export interface Profile {
  researchAreas: string[];
  otherArea: string | null;
  background: string | null;
  papers: LinkedPaper[];
}

export const getProfile = () => get<Profile>("/api/settings/profile");

export const setProfile = (body: Profile) => post<Profile>("/api/settings/profile", body);

/** Which literature sources discovery and paper reading may use (settings.json). */
export interface LitSourcesSettings {
  alphaxiv: boolean;
  openalex: boolean;
  biorxiv: boolean;
}

export const getLitSources = () =>
  get<LitSourcesSettings>("/api/settings/lit-sources");

export const setLitSources = (body: LitSourcesSettings) =>
  post<LitSourcesSettings>("/api/settings/lit-sources", body);

export interface ProjectDefaultsSettings {
  githubForNewProjects: boolean;
  githubDefaultPromptSeen: boolean;
  ghInstalled: boolean;
  githubAuthenticated: boolean;
}

export const getProjectDefaults = () =>
  get<ProjectDefaultsSettings>("/api/settings/projects");

export const setProjectDefaults = (
  githubForNewProjects: boolean,
  githubDefaultPromptSeen?: boolean,
) =>
  post<ProjectDefaultsSettings>("/api/settings/projects", {
    githubForNewProjects,
    ...(githubDefaultPromptSeen === undefined ? {} : { githubDefaultPromptSeen }),
  });

export interface ProjectGitStatus {
  path: string;
  gitVersion: string | null;
  initialized: boolean;
  baselineBranch: string;
  currentBranch: string | null;
  clean: boolean | null;
  remotes: { name: string; url: string }[];
  identity: {
    name: string | null;
    email: string | null;
    nameSource: "local" | "global" | null;
    emailSource: "local" | "global" | null;
  };
  github: {
    ghInstalled: boolean;
    authenticated: boolean;
    enabled: boolean;
    owner: string;
    repo: string;
    url: string | null;
    syncStatus: string | null;
  };
}

export const getProjectGitStatus = (projectId: string) =>
  get<ProjectGitStatus>(`/api/projects/${projectId}/git`);

export const initializeProjectGit = (projectId: string) =>
  post<ProjectGitStatus>(`/api/projects/${projectId}/git/init`);

export const enableProjectGithub = (projectId: string) =>
  post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github`);

export const disableProjectGithub = (projectId: string) =>
  post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github/disable`);

export const pushProjectGithub = (projectId: string) =>
  post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github/push`);

export interface TelemetrySettings {
  /** Whether usage analytics linked to the random installation ID is on. */
  enabled: boolean;
  /** Saved user preference, independent of build and runtime eligibility. */
  preferenceEnabled: boolean;
  /** Whether the current build or launch configuration forces analytics off. */
  locked: boolean;
  /** When off, a short human reason (e.g. "--no-telemetry flag"); null when on. */
  reason: string | null;
}

export const getTelemetry = () => get<TelemetrySettings>("/api/settings/telemetry");

export const setTelemetry = (enabled: boolean) =>
  post<TelemetrySettings>("/api/settings/telemetry", { enabled });

export type HarnessId = "claude-code" | "codex" | "opencode";

export interface HarnessModel {
  id: string;
  /**
   * Reasoning/effort choices this *specific* model accepts, led by the
   * `default` sentinel. Absent means "no list of its own" — fall back to the
   * harness-wide {@link HarnessOptions.reasoningLevels}. An empty array is
   * different: the model was checked and genuinely has no reasoning control,
   * so the picker is hidden. Use `reasoningFor` rather than reading this
   * directly.
   */
  reasoningLevels?: OptionChoice[];
  /** The catalog's own human name ("Opus", "GPT-5.6 Sol"). Absent on
   * statically-listed fallback models — derive from the id then. */
  displayName?: string;
  /** The catalog's one-line blurb. For Claude this carries the resolved
   * version ("Opus 4.8 with 1M context · …") — its aliases don't. */
  description?: string;
  /** The tier that actually runs when nothing is chosen — present only when
   * the CLI reports it (codex). When set, `reasoningLevels` has no `default`
   * sentinel and the composer preselects this concrete tier. */
  defaultReasoningLevel?: string;
  /** Additional processing tiers this model advertises (Codex Fast mode). */
  serviceTiers?: OptionChoice[];
}

/** Display label for a harness model: the catalog's own name when it has one,
 * else prettified from the id. */
export const harnessModelLabel = (m: HarnessModel) => m.displayName ?? modelLabel(m.id);

/** One selectable value in a composer toggle (permission mode / reasoning). */
export interface OptionChoice {
  id: string;
  label: string;
  description?: string;
}

/**
 * The toggle vocabulary a harness supports. Empty arrays hide the control.
 *
 * `reasoningLevels` here is the harness-wide *fallback*; per-model choices ride
 * on {@link HarnessModel.reasoningLevels} and win. Resolve with `reasoningFor`.
 */
export interface HarnessOptions {
  permissionModes: OptionChoice[];
  defaultPermissionMode?: string | null;
  planActivation?: "permission" | "command" | null;
  reasoningLevels: OptionChoice[];
  defaultReasoningLevel?: string | null;
}

/**
 * Wire id for "send no explicit effort/variant — let the CLI and its own config
 * decide". Must match `REASONING_DEFAULT_ID` in `src/local/harness/options.rs`.
 */
export const REASONING_DEFAULT_ID = "default";

/**
 * The reasoning choices to show for a harness + model, and the id to treat as
 * the default.
 *
 * Per-model metadata wins when present (Codex's per-model tiers, OpenCode's
 * `variants`); otherwise the harness-wide list applies. A model that reports an
 * empty list genuinely has no reasoning control, so the picker is hidden — this
 * is why the absent/empty distinction matters and `?? []` would be wrong.
 */
export function reasoningFor(
  harness: Harness | undefined,
  modelId: string | null | undefined,
): { choices: OptionChoice[]; defaultId: string | null } {
  const model = harness?.models.find((m) => m.id === modelId);
  const choices = model?.reasoningLevels ?? harness?.options?.reasoningLevels ?? [];
  // Preselection, in order of how much the CLI actually told us:
  //  1. the model's reported concrete default (codex) — a real tier, shown as
  //     the value that will run;
  //  2. the `default` sentinel when it's on offer — "send no override", for
  //     harnesses whose unset default isn't any fixed tier (Claude's adaptive
  //     thinking) or is unknown (opencode);
  //  3. the harness-wide default, then the first tier.
  const modelDefault = model?.defaultReasoningLevel;
  const defaultId =
    modelDefault && choices.some((c) => c.id === modelDefault)
      ? modelDefault
      : choices.some((c) => c.id === REASONING_DEFAULT_ID)
        ? REASONING_DEFAULT_ID
        : (harness?.options?.defaultReasoningLevel ?? choices[0]?.id ?? null);
  return { choices, defaultId };
}

export const SERVICE_TIER_DEFAULT_ID = "default";

export function serviceTiersFor(
  harness: Harness | undefined,
  modelId: string | null | undefined,
): OptionChoice[] {
  if (harness?.id !== "codex") return [];
  const tiers = harness.models.find((model) => model.id === modelId)?.serviceTiers;
  if (!tiers?.length) return [];
  return [
    { id: SERVICE_TIER_DEFAULT_ID, label: m.service_tier_standard(), description: m.service_tier_default_speed() },
    ...tiers,
  ];
}

export function reconcileServiceTier(
  harness: Harness | undefined,
  modelId: string | null | undefined,
  current: string | null | undefined,
): string | null {
  // Harness detection is async; preserve a deliberate choice until its catalog loads.
  if (!harness) return current ?? null;
  if (harness.id !== "codex") return null;
  const tiers = harness.models.find((model) => model.id === modelId)?.serviceTiers;
  if (tiers === undefined) return null;
  const choices = serviceTiersFor(harness, modelId);
  if (choices.length === 0) return SERVICE_TIER_DEFAULT_ID;
  return current != null && choices.some((choice) => choice.id === current)
    ? current
    : SERVICE_TIER_DEFAULT_ID;
}

/**
 * Keep a stored reasoning level only if the given harness+model still offers
 * it; otherwise fall back to that model's default. This is what makes switching
 * models drop an effort the new model can't accept (e.g. `ultra` off Sol onto
 * 5.5) instead of silently sending an invalid value.
 */
export function reconcileReasoning(
  harness: Harness | undefined,
  modelId: string | null | undefined,
  current: string | null,
): string | null {
  // Harness not resolved yet — detection is async, and it can also fail. "We
  // don't know what this model offers" is not "this model offers nothing", so
  // leave the stored level alone; resetting it here would let a send that races
  // the harness fetch overwrite a deliberate choice.
  if (!harness) return current;
  const { choices, defaultId } = reasoningFor(harness, modelId);
  // A model with no reasoning control: the picker is hidden, so the user can no
  // longer clear a level themselves. Return the sentinel rather than `null` —
  // `null` reads as "no override supplied" and leaves whatever the session row
  // already holds in place, which would keep sending a stale level the model
  // never offered.
  if (choices.length === 0) return REASONING_DEFAULT_ID;
  return current && choices.some((c) => c.id === current) ? current : defaultId;
}

export interface Harness {
  id: HarnessId;
  name: string;
  installed: boolean;
  /** On PATH, but `--version` failed — a broken install a reinstall repairs.
   * Never `agentReady`: spawning it just dumps the CLI's own crash into chat. */
  installBroken: boolean;
  binPath?: string;
  version?: string;
  authenticated: boolean;
  authState: "ready" | "needsLogin" | "unknown" | "unsupported";
  authMethod?: "oauth" | "apiKey";
  account?: string;
  org?: string;
  plan?: string;
  agentReady: boolean;
  agentNote?: string;
  /** A running turn takes further input, so the composer steers instead of
   * queueing. Narrowed per installation (codex's legacy exec path can't). */
  supportsSteering: boolean;
  models: HarnessModel[];
  options: HarnessOptions;
}

export const getHarnesses = (refresh = false, retryRejected = false) => {
  const params = new URLSearchParams();
  if (refresh) params.set("refresh", "1");
  if (retryRejected) params.set("retry", "1");
  const query = params.size > 0 ? `?${params.toString()}` : "";
  return get<{ harnesses: Harness[] }>(`/api/harnesses${query}`).then((r) => r.harnesses);
};

/** Slash-skill offered in the composer's `/` dropdown; expanded server-side. */
export interface SkillInfo {
  name: string;
  description: string;
  /** Built-in composer commands share the menu with harness/user skills. */
  source?: "builtin" | "user" | "command";
}

export const getSkills = () => get<{ skills: SkillInfo[] }>("/api/skills").then((r) => r.skills);

export const getSkillContent = (name: string, projectId?: string) =>
  get<{ content: string }>(
    `/api/skills/${encodeURIComponent(name)}${
      projectId ? `?project=${encodeURIComponent(projectId)}` : ""
    }`,
  ).then((r) => r.content);

/** A user-uploaded LaTeX template the paper skill follows, managed in the
 * Customize tab. */
export interface LatexTemplate {
  name: string;
  /** Relative path of the .tex the agent starts from. */
  entry: string;
  /** Class/style files shipped alongside it. */
  supportFiles: string[];
  bytes: number;
  updatedAt: number;
}

export const listLatexTemplates = () =>
  get<{ templates: LatexTemplate[] }>("/api/latex-templates").then((r) => r.templates);

export const uploadLatexTemplate = (body: { filename: string; contentBase64: string }) =>
  post<{ template: LatexTemplate }>("/api/latex-templates", body).then((r) => r.template);

export const deleteLatexTemplate = (name: string) =>
  fetch(`/api/latex-templates?name=${encodeURIComponent(name)}`, { method: "DELETE" }).then((r) =>
    json<{ ok: boolean }>(r),
  );

/** A skill the agent gets in every session: a SKILL.md folder uploaded in the
 * Customize tab, or one mirrored from an installed coding agent or its plugins. */
export interface UserSkill {
  name: string;
  /** The coding agent or plugin it comes from; absent when uploaded here, which
   * is also the only case the user can delete. */
  origin?: string | null;
  bytes: number;
  updatedAt: number;
}

export const listUserSkills = () =>
  get<{ skills: UserSkill[] }>("/api/user-skills").then((r) => r.skills);

/** Upload a SKILL.md file or a .zip of a skill folder. `contentBase64` is the
 * raw file bytes; `filename`'s extension selects single-file vs archive. */
export const uploadUserSkill = (req: { filename: string; contentBase64: string }) =>
  post<{ skill: UserSkill }>("/api/user-skills", req).then((r) => r.skill);

export const deleteUserSkill = (name: string) =>
  fetch(`/api/user-skills?name=${encodeURIComponent(name)}`, { method: "DELETE" }).then((r) =>
    json<{ ok: boolean }>(r),
  );

/** "openai/gpt-5.5" → "GPT 5.5", "anthropic/claude-opus-4-8" → "Opus 4.8". */
export function modelLabel(id: string): string {
  const last = (id.split("/").pop() ?? id).replace(/^~/, "").replace(/^claude-/, "");
  const words: string[] = [];
  const nums: string[] = [];
  for (const part of last.split("-")) {
    if (/^\d+(\.\d+)?$/.test(part)) {
      nums.push(part);
    } else {
      if (nums.length) words.push(nums.splice(0).join("."));
      words.push(part === "gpt" ? "GPT" : part.charAt(0).toUpperCase() + part.slice(1));
    }
  }
  if (nums.length) words.push(nums.join("."));
  return words.join(" ");
}

// --- chat (unified harness sessions) ------------------------------------------

export interface ChatToolState {
  status: "running" | "completed" | "error";
  input?: {
    command?: string;
    commandArgv?: string[];
    filePath?: string;
    description?: string;
    retryOwner?: "native" | "orx";
    attempt?: number;
    maximum?: number | null;
    nextRetryAt?: number | null;
    turnId?: string;
    errorKind?: string;
    recoveryAction?: "retry" | "continue";
    [k: string]: unknown;
  };
  output?: string;
  error?: string;
  title?: string;
}

export interface ChatQuestionOption {
  label: string;
  description?: string;
}

/** An interactive request the user acts on before the harness continues. */
export interface ChatPrompt {
  kind: "plan" | "permission" | "question";
  resolved: boolean;
  plan?: string;
  /** plan: card synthesized from the turn's final text (no ExitPlanMode call). */
  synthesized?: boolean;
  tool?: string;
  toolInput?: Record<string, unknown>;
  question?: string;
  header?: string;
  options?: ChatQuestionOption[];
  multiSelect?: boolean;
  planExit?: boolean;
  /** Answer echo, stamped on resolve: chosen labels (questions), whether the
   * card was approved (plan/permission), and any freeform note. Absent on
   * cards resolved without an answer (stale-card cleanup). */
  answers?: string[];
  approved?: boolean;
  note?: string;
  annotations?: ChatTextAnnotation[];
  /** Backend resume routing id. Presence marks a HELD mid-turn card (the
   * turn is blocked open waiting on this answer); absent on end-turn cards. */
  nativeId?: string;
}

export interface ChatPart {
  id: string;
  type: string; // text | reasoning | tool | prompt | image | steer
  text?: string;
  /** Original file name for an `image` (attachment) part, when known. */
  name?: string;
  tool?: string;
  state?: ChatToolState;
  prompt?: ChatPrompt;
  /** Nested transcript of a sub-agent this part spawned (Codex `subagent`
   * tool). Streams live and recurses for sub-agents that spawn their own. */
  children?: ChatPart[];
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  parts: ChatPart[];
  createdAt: number;
  parentId?: string | null;
}

/** How much of the model's context window a session has used, measured off the
 * most recent API request (latest wins, not cumulative). `contextWindow` is
 * absent when the harness doesn't report one. */
export interface ContextUsage {
  usedTokens: number;
  contextWindow?: number;
}

export interface ChatSession {
  id: string;
  projectId: string;
  harness: HarnessId;
  title: string | null;
  /** Who wrote `title`: `"fallback"` (first-line placeholder), `"generated"`
   * (harness auto-title), `"user"` (explicitly chosen — a rename, or an agent's
   * `orx agent spawn --title`). Null on legacy sessions. */
  titleSource?: string | null;
  model: string | null;
  serviceTier: string | null;
  permissionMode: string | null;
  /** Independent Plan axis for Codex/OpenCode. */
  planMode: boolean;
  reasoningLevel: string | null;
  /** Hidden from the default Recents list, but fully intact and resumable. */
  archived: boolean;
  /** Session whose agent spawned this one with `orx agent spawn`; null for
   * sessions the user started themselves. */
  parentSessionId?: string | null;
  createdAt: number;
  updatedAt: number;
  busy: boolean;
  contextUsage?: ContextUsage;
}

export const listChatSessions = (projectId: string) =>
  get<{ sessions: ChatSession[] }>(
    `/api/chat/sessions?projectId=${encodeURIComponent(projectId)}`,
  ).then((r) => r.sessions);

/** Per-session (and per-turn) composer selections beyond the harness itself. */
export interface TurnOptions {
  model?: string | null;
  serviceTier?: string | null;
  permissionMode?: string | null;
  planMode?: boolean;
  reasoningLevel?: string | null;
}

export const createChatSession = (
  projectId: string,
  harness: HarnessId,
  opts: TurnOptions = {},
) =>
  post<{ session: ChatSession }>("/api/chat/sessions", { projectId, harness, ...opts }).then(
    (r) => r.session,
  );

export const deleteChatSession = (sessionId: string) =>
  fetch(`/api/chat/sessions/${sessionId}`, { method: "DELETE" }).then((r) =>
    json<{ ok: boolean }>(r),
  );

/** Archive/unarchive a session (archived chats stay resumable). */
export const setChatSessionArchived = (sessionId: string, archived: boolean) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { archived }).then(
    (r) => r.session,
  );

/** Rename a session. The title is trimmed server-side; empty titles are rejected. */
export const renameChatSession = (sessionId: string, title: string) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { title }).then(
    (r) => r.session,
  );

/** Enter/leave the session-specific Plan axis used by Codex/OpenCode. */
export const setChatSessionPlanMode = (sessionId: string, planMode: boolean) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { planMode }).then(
    (r) => r.session,
  );

export const setChatSessionPermissionMode = (sessionId: string, permissionMode: string) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { permissionMode }).then(
    (r) => r.session,
  );

/** A message the user sent while a turn was running, parked to run next. */
export interface QueuedMessage {
  id: string;
  text: string;
  planMode?: boolean;
  dispatchState?: "queued" | "retrying" | "blocked";
  nextRetryAt?: number | null;
  error?: string | null;
}

export const getChatMessages = (sessionId: string) =>
  get<{ messages: ChatMessage[]; queued?: QueuedMessage[]; activeLeafId?: string | null }>(
    `/api/chat/sessions/${sessionId}/messages`,
  ).then((r) => ({
    messages: r.messages,
    queued: r.queued ?? [],
    activeLeafId: r.activeLeafId ?? null,
  }));

/** Remove a still-parked message. */
export const cancelQueuedMessage = (sessionId: string, itemId: string) =>
  fetch(`/api/chat/sessions/${sessionId}/queue/${encodeURIComponent(itemId)}`, {
    method: "DELETE",
  }).then((r) => json<{ ok: boolean; removed: boolean }>(r));

/** Retry the same parked message after safe queue delivery was exhausted. */
export const retryQueuedMessage = (sessionId: string, itemId: string) =>
  post<{ ok: boolean; retried: boolean }>(
    `/api/chat/sessions/${sessionId}/queue/${encodeURIComponent(itemId)}`,
  );

/** A pasted image or uploaded file riding a chat message. */
export interface ChatImageAttachment {
  mediaType: string;
  dataBase64: string;
  /** Original file name (uploads/drops); pasted images carry none. */
  name?: string;
}

export interface ChatTextAnnotation {
  text: string;
}

/** Image parts store a server-minted file name; this is where it's served. */
export const chatAttachmentUrl = (name: string) =>
  `/api/chat/attachments/${encodeURIComponent(name)}`;

/** Returns immediately; the turn streams over /api/events (chat.* events). */
export const sendChatMessage = (
  sessionId: string,
  text: string,
  opts: TurnOptions = {},
  images?: ChatImageAttachment[],
  annotations?: ChatTextAnnotation[],
  clientTurnId?: string,
  mode?: "steer",
) =>
  post<{ ok: boolean; turn?: ChatTurnResult; steered?: boolean }>(
    `/api/chat/sessions/${sessionId}/message`, {
    text,
    clientTurnId,
    model: opts.model,
    serviceTier: opts.serviceTier,
    permissionMode: opts.permissionMode,
    planMode: opts.planMode,
    reasoningLevel: opts.reasoningLevel,
    images,
    annotations,
      mode,
    },
  );

/** A composer `!` command, run in the session's checkout and recorded on its
 * transcript as a user-side exchange the next turn is told about. */
export const runShellCommand = (sessionId: string, command: string) =>
  post<{ message: ChatMessage }>(`/api/chat/sessions/${sessionId}/shell`, { command });

export interface ChatTurnResult {
  turnId: string;
  queued: boolean;
  existing: boolean;
}

export const recoverChatTurn = (
  sessionId: string,
  turnId: string,
  action: "retry" | "continue",
  opts: TurnOptions = {},
) =>
  post<{ ok: boolean; turn: ChatTurnResult }>(
    `/api/chat/sessions/${sessionId}/turns/${turnId}/recover`,
    { action, ...opts },
  );

/** Pass `text` to re-ask an edited version of a user message; omit it to retry a
 * response. Returns immediately; the new turn streams over /api/events. */
export const forkChatTurn = (sessionId: string, messageId: string, text?: string) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/fork`, { messageId, text });

/** Show a different fork of a turn, along with the whole branch under it. */
export const selectChatBranch = (sessionId: string, leafId: string) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/branch`, { leafId });

export const interruptChat = (sessionId: string) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/interrupt`);

/** Answer an interactive prompt (plan / permission / question) on a session. */
export interface PromptAnswer {
  promptId: string;
  approve?: boolean;
  /** Permission mode to resume under (plan/permission approval). */
  resumeMode?: string;
  /** Chosen option labels (questions). */
  answers?: string[];
  note?: string;
  annotations?: ChatTextAnnotation[];
}

export const respondChat = (sessionId: string, answer: PromptAnswer) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/respond`, answer);

// --- helpers shared across views --------------------------------------------

export function statusColor(status: string): string {
  switch (status) {
    case "done":
      return "var(--green)";
    case "running":
      return "var(--teal)";
    case "starting":
      return "var(--amber)";
    case "failed":
      return "var(--red)";
    case "cancelled":
      return "var(--muted)";
    default:
      return "var(--muted)";
  }
}

export function timeAgo(ms: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  const format = new Intl.RelativeTimeFormat(getLocale(), { numeric: "always", style: "narrow" });
  if (s < 60) return format.format(-s, "second");
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return format.format(-minutes, "minute");
  const h = Math.floor(minutes / 60);
  if (h < 24) return format.format(-h, "hour");
  return format.format(-Math.floor(h / 24), "day");
}

/** "42s" / "18m" / "2h 28m" / "1d 4h" — an elapsed duration, not a timestamp. */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return m.duration_seconds({ value: fmtNumber(s) });
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return m.duration_minutes({ value: fmtNumber(minutes) });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return m.duration_hours_minutes({ hours: fmtNumber(hours), minutes: fmtNumber(minutes % 60) });
  return m.duration_days_hours({ days: fmtNumber(Math.floor(hours / 24)), hours: fmtNumber(hours % 24) });
}

/** Compact byte size, e.g. "512 B", "2.0 KB", "5.3 MB". Mirrors the backend's
 * `store::human_bytes`. */
export function fmtBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  const number = new Intl.NumberFormat(getLocale(), { maximumFractionDigits: u === 0 ? 0 : 1, minimumFractionDigits: u === 0 ? 0 : 1 });
  return `${number.format(u === 0 ? n : v)} ${units[u]}`;
}

/** Compact token count, e.g. 62300 → "62k", 1_200_000 → "1.2M", 940 → "940". */
export function fmtTokens(n: number): string {
  const number = new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 1 });
  if (n < 1000) return number.format(Math.round(n));
  if (n < 999_950) return `${number.format(n / 1000)}k`;
  return `${number.format(n / 1_000_000)}M`;
}

export function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 10)}…` : id;
}

/** The backend kind from a run's `backend` descriptor ("modal_job", "hf_job", …). */
export function backendKind(backend: Run["backend"]): string {
  if (!backend) return "";
  if (typeof backend.kind === "string") return backend.kind;
  if (typeof backend.type === "string") return backend.type;
  return "";
}

/** The flavor / manifest / host that qualifies a backend, if any. k8s runs
 *  carry a manifest path instead of a flavor; ssh a host in `namespace`. */
export function backendDetail(backend: Run["backend"]): string {
  if (!backend) return "";
  if (typeof backend.flavor === "string" && backend.flavor) return backend.flavor;
  if (typeof backend.manifest === "string" && backend.manifest) return backend.manifest;
  // Ray's namespace is the whole Jobs URL — too long for a badge.
  if (backendKind(backend) === "ray_job") return "";
  if (typeof backend.namespace === "string" && backend.namespace) return backend.namespace;
  return "";
}
