import { LogPageUnavailable } from "./logWindow";
// Typed client for the orx up local HTTP API (/api/*). All wire JSON is camelCase.

import { ConnectionLostError, type WorkspaceClient } from "./queries/client";
import type { WorkspaceInvalidation } from "./queries/invalidation";

import type { GlobalWorkspace, ProjectWorkspace } from "./workspaceState";
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

/** Stable analytics labels for the bundled demo's recorded conversations. */
export const DEMO_EXPERIMENT_LABELS: Record<string, string> = {
  [DEMO_MAIN_SESSION_ID]: "cpu_end_to_end",
  [DEMO_FIGURE_SESSION_ID]: "figures",
  [DEMO_LITERATURE_SESSION_ID]: "literature",
};
export const DEMO_OVERVIEW_ARTIFACT = "cpu-apple-silicon-pipeline-results.md";
/** Leaf message each recorded demo session is seeded with; a send moves it. */
export const DEMO_SEEDED_LEAF_IDS: Record<string, string> = {
  [DEMO_MAIN_SESSION_ID]: "msg_demo_nanochat_assistant_v1",
  [DEMO_FIGURE_SESSION_ID]: "msg_demo_nanochat_figures_assistant_v1",
  [DEMO_LITERATURE_SESSION_ID]: "msg_demo_nanochat_literature_assistant_v1",
};
export const DEMO_RUN_EXPERIMENT_PROMPT =
  "Run the proposed learning-rate probe experiment. Summarize its results using the recorded validation metric and comparison described by the experiment. If it produces an interactive HTML loss chart, show it inline using image syntax.";

export class FileChangedError extends Error {
  readonly currentVersion: string | null;
  readonly exists: boolean;

  constructor(
    message: string,
    currentVersion: string | null,
    exists: boolean,
  ) {
    super(message);
    this.name = "FileChangedError";
    this.currentVersion = currentVersion;
    this.exists = exists;
  }
}

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
  /** arXiv or alphaXiv paper id the project starts from (versionless). */
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
  archived: boolean;
}

/** An experiment a chat created, launched a run of, or asked to be woken for. */
export interface SessionExperiment {
  experiment: Experiment;
  projectName: string;
  /** Its newest live run, else its newest run: a forced relaunch can leave an older run live. Null until it has been run. */
  currentRun: (Pick<Run, "status" | "createdAt" | "endedAt" | "contact" | "cancelling" | "cancelUnconfirmed" | "host" | "lostReason"> & { cancelRequested: boolean }) | null;
}

export type RunStatus = "submitting" | "starting" | "running" | "done" | "failed" | "cancelled" | "lost";

/** How fresh the engine's knowledge of a run is; null means live. */
export type RunContact =
  | { type: "unreachable" | "needs_auth"; since: number; cause: string }
  | { type: "unreconciled"; since: number; detail: string }
  | { type: "stopping"; since: number; count: number };

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
  startedAt?: number | null;
  endedAt?: number | null;
  exitCode?: number | null;
  contact: RunContact | null;
  cancelling: boolean;
  cancelUnconfirmed?: number[] | null;
  /** The backend's own word on why a submitting or starting run hasn't started. */
  waitingReason?: string | null;
  /** Why the stop of a cancelling run keeps failing to reach its backend, which retries it. */
  cancelError?: string | null;
  lostReason: string | null;
  /** The SSH alias or Slurm login node the run's job lives on. */
  host: string | null;
  /** Why the run's latest notice batch was given up on, never reaching its chat. */
  noticeError?: string | null;
  /** Copying a finished run's log to this computer; both are absent once the log is kept. */
  logCollection?: "pending" | "failed";
  logCollectionError?: string;
}

export class RejectedResponseError extends Error {}

/** OpenCode 1 can't do this; updating it or using OpenResearch's copy can. */
export class OpenCode1Error extends RejectedResponseError {}

/** Why a model server listed no models: nothing answered, it wants an API key, or it has no models. */
export class UnlistedError extends RejectedResponseError {
  constructor(message: string, readonly code: "unreachable" | "needsKey" | "noModels") {
    super(message);
  }
}

/** A rejected save whose messages each belong to one form field, keyed as in the request body. */
export class FieldErrors extends RejectedResponseError {
  constructor(message: string, readonly fields: Record<string, string[]>) {
    super(message);
  }
}

export interface ProjectActivity {
  projectId: string;
  activeAgents: number;
  /** Lifetime sessions for this project, including archived agents. */
  totalAgents: number;
  runningExperiments: number;
  totalExperiments: number;
  lastMessageAt: number | null;
  lastActivityAt: number;
}

export interface OnboardingSelection {
  harness: HarnessId;
  /** An account OpenResearch keeps for the agent; absent for the user's own sign-in. */
  profile?: string | null;
  model: string | null;
  serviceTier?: string | null;
  permissionMode: string | null;
  reasoningLevel: string | null;
}

export type AgentSelection = OnboardingSelection;

export interface UiState {
  workspace: GlobalWorkspace | null;
  onboardingCompleted: boolean;
  tourCompleted: boolean;
  preferredAgent: AgentSelection | null;
  preferredAutonomy: Autonomy | null;
  /** Whether sub-agents may start their own unless a delegation says otherwise. */
  nestedDelegation: boolean;
  /** Whether OpenCode's free models are offered beside the user's own. */
  opencodeFreeModels: boolean;
  /** The model families the picker lists first. */
  favoriteModels: string[];
  /** Models turned off in Settings, by {@link ModelName.key}: the picker hides them and agents can't delegate to them. */
  hiddenModels: string[];
  /** Agents found on this computer that are turned off, by harness id; `opencode` stands for the sign-ins made in OpenCode itself. */
  hiddenAgents: string[];
  /** The picker's lab sections in the user's order; labs it doesn't name follow in the picker's own. */
  modelSectionOrder: string[];
  collapsedModelSections: string[];
  modelSightings: ModelSightings | null;
}

/** What the model picker has seen, to tell a model an agent newly offers from one that came with a newly added agent. */
export interface ModelSightings {
  /** The agents whose models were seen, by `PickerAgent.key`. */
  agents: string[];
  /** Each model seen, by line key: when it first appeared as new, or 0 for one that was there already. */
  models: Record<string, number>;
  /** When the picker was last opened. */
  openedAt: number;
}

/** How much of the research the agent owns before checking in. */
export type Autonomy = "copilot" | "agentic";
export const DEFAULT_AUTONOMY: Autonomy = "agentic";

export interface ProjectPathStatus {
  gitVersion: string | null;
  /** Only without a path: Windows with no Git, which `installGit` can fix. */
  gitInstallable?: boolean;
  resolvedPath: string | null;
  exists: boolean | null;
  directory: boolean | null;
  empty: boolean | null;
  initialized: boolean | null;
  gitState?: "notRepository" | "unborn" | "ready" | "detached" | "invalid" | null;
  githubOwner?: string | null;
  githubRepo?: string | null;
}

export interface NewProject {
  name: string;
  path: string;
  runCommand?: string;
  paperId?: string;
  cloneUrl?: string;
  creationMode?: "blank" | "folder" | "paper";
  createFolder?: boolean;
  requireNewFolder?: boolean;
  initializeGit?: boolean;
  githubSyncEnabled?: boolean;
  /** UI locale for the starter prompts the server warms up on creation. */
  locale?: string;
  starterAgent?: Pick<AgentSelection, "harness" | "model" | "profile">;
}

export interface CreateProjectResult {
  project: Project;
  githubPublicationError: string | null;
}

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

/** One suggested opening message for the empty chat: written by a model that
 *  read the project, or pre-written when the project is blank. */
export interface StarterPrompt {
  title: string;
  prompt: string;
}

export interface ProjectStarterPrompts {
  /** Empty when the project already has experiments or is blank; null when
   *  the harness could not answer. */
  prompts: StarterPrompt[] | null;
  /** Use pre-written prompts when there is no project brief or no selected
   *  OpenCode model for generation. */
  blank: boolean;
}

/** A run viewed as compute: every run across all projects, tagged with the
 *  name of the project that launched it. `projectName` is enriched only on the
 *  /api/instances snapshot — it is absent from the `run.updated` SSE payload. */
export interface Instance extends Run {
  projectName?: string;
}

/** A log file's identity on its host, opaque: a different one means the file was replaced. */
export type LogIdent = string;

export type LogPage =
  | {
    kind: "data";
    ident: LogIdent;
    start: number;
    /** Base64. */
    data: string;
    eof: boolean;
    /** Null while a live run's log is still growing on its host. */
    len: number | null;
    /** Byte ranges kept on this computer; the rest is read from the host on demand. */
    kept: [number, number][];
  }
  | { kind: "reset"; info: { ident: LogIdent; len: number } | null };

export type HostStatus = "idle" | "connecting" | "live" | "backoff" | "needs_auth";

/** The run engine's connection to one SSH alias. */
export interface EngineHost {
  alias: string;
  /** The run directory root this connection serves; one alias can have several. */
  root: string;
  status: HostStatus;
  class: "key" | "interactive" | null;
  /** The old launcher on Windows logs in only through Connect, even with a key. */
  connectRequired: boolean;
  sinceMs: number;
  nextRetryMs: number | null;
  cause: string | null;
}

export interface DiffPayload {
  diff: string;
  truncated: boolean;
  bytesRead: number;
  byteLimit: number;
}

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
  /** Exact live-checkout bytes; null for read-only/incomplete sources and
   * absent when connected to an older remote server. */
  version?: string | null;
}

/** A file read by absolute path, outside the project's checkout and artifacts
 * (e.g. `/Users/me/.ssh/config`). Same capped/decoded body as `ProjectFile`;
 * the wire `root` is always `"abs"`, so it's dropped here rather than widening
 * `CheckoutRoot` — nothing reads it, and it isn't part of any git tree. */
export type AbsoluteFile = Omit<ProjectFile, "root">;

export type FileAction =
  { action: "rename"; newName: string } | { action: "duplicate" | "delete" };

export interface FileActionResult {
  ok: boolean;
  path: string;
}

export interface LatexEngine {
  /** The engine that will run, or null when the machine has none. */
  engine: string | null;
  /** Install guidance, present only when `engine` is null. */
  hint: string | null;
  /** A paste-ready install command, where this platform has one. */
  installCommand: string | null;
}

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

/** The Overleaf project a `.tex` pushes to. */
export interface OverleafLink {
  projectId: string;
  /** The Overleaf site the project lives on: www.overleaf.com, or a Server Pro host. */
  host: string;
  /** The project on overleaf.com, for opening it. */
  url: string;
}

export interface OverleafState {
  /** A Git authentication token is stored on this machine. */
  hasToken: boolean;
  /** A browser session cookie is stored, so a linked paper can follow
   * Overleaf live rather than by polling. */
  hasSession: boolean;
  /** Null until this paper is pointed at a project. */
  link: OverleafLink | null;
}

export interface OverleafSettings {
  hasToken: boolean;
  hasSession: boolean;
}

export interface OverleafLiveStatus {
  state: "connecting" | "live" | "stopped";
  /** Why the channel stopped and will not reconnect on its own. */
  error: string | null;
  /** The cookie is what it stopped over, so a fresh one is the way back in. */
  needsSession: boolean;
  /** Why some documents are not moving: read-only access, a conflict held
   * for the git sync, a format this client cannot follow. */
  note: string | null;
}

export interface OverleafLive {
  /** Identifies this session in `overleaf.live` and `overleaf.pulled` events. */
  key: string;
  /** Null in a stop's reply; a start always reports the channel's state. */
  status: OverleafLiveStatus | null;
}

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

export interface CodeTree {
  root: CheckoutRoot;
  /** Absolute live checkout path; absent for committed branch listings. */
  path?: string;
  /** The listed branch (`ref` mode), else the checked-out branch, else null
   * (detached HEAD). */
  branch: string | null;
  /** Repo-relative file paths (gitignored trees excluded), sorted. */
  entries: string[];
  /** True when the listing hit the server-side cap (20,000 entries). */
  truncated: boolean;
}

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

/** A GitHub `tree` URL for a branch. Branch names contain `/` (`orx/<slug>`),
 * so encode each path segment — never the whole string, which would escape the
 * slashes. Unpushed branches 404 on GitHub, which is acceptable. */
export const githubBranchUrl = (owner: string, repo: string, branch: string) =>
  `https://github.com/${owner}/${repo}/tree/${branch
    .split("/")
    .map(encodeURIComponent)
    .join("/")}`;

export type HfTokenSource = "env" | "openresearchEnv" | "hfCache";
export type HfValidationStatus = "missing" | "valid" | "invalid" | "unreachable";

export interface HfSettings {
  configured: boolean;
  source: HfTokenSource | null;
  maskedToken: string | null;
  validationStatus: HfValidationStatus;
  validationError: string | null;
  username: string | null;
  jobsWrite: boolean | null;
  sourceStaging: boolean | null;
  sourceStagingError: string | null;
}

export interface TinkerSettings {
  maskedKey: string | null;
  validationStatus: "missing" | "valid" | "invalid" | "billingRequired";
  processEnv: boolean;
}

// --- updates ------------------------------------------------------------------

/** How orx was installed. `installer`, `app-bundle`, `appimage` and `portable` update themselves. */
export type InstallChannel =
  | "installer"
  | "app-bundle"
  | "appimage"
  | "portable"
  | "cargo"
  | "homebrew"
  | "nix"
  | "unknown";
export type ReleaseChannel = "stable" | "canary";

/** Commit titles by conventional prefix. */
export interface Changes {
  features: number;
  fixes: number;
  other: number;
}

/** What a canary build holds beyond the latest stable release; null for any other build or when none were published. */
export interface BuildNotes {
  base: string;
  /** Commit titles, newest first. */
  commits: string[];
}

/** What quitting does to a pending experiment. */
export type AfterQuit = "reports" | "continues" | "needsApp";

export interface PendingAgent {
  projectId: string;
  project: string | null;
  sessionId: string;
  sideParentId: string | null;
  harness: HarnessId;
  title: string | null;
}

export interface PendingRun {
  projectId: string;
  project: string | null;
  experimentId: string;
  runId: string;
  sessionId: string | null;
  name: string;
  backend: string | null;
  afterQuit: AfterQuit;
}

/** What a quit would interrupt on this machine. */
export interface PendingWork {
  agents: PendingAgent[];
  runs: PendingRun[];
}

export interface UpdateStatus {
  current: string;
  /** Latest published release on the selected channel. */
  latest: string | null;
  latestTag: string | null;
  channel: InstallChannel;
  releaseChannel: ReleaseChannel;
  releaseChannelsAvailable: boolean;
  waitingForStable: boolean;
  waitingForCanary: boolean;
  /** Whether this install is one orx can replace at all. */
  selfUpdates: boolean;
  autoUpdate: boolean;
  /** `autoUpdate` is off because of the environment, not the user's setting. */
  envDisabled: boolean;
  updateAvailable: boolean;
  /** What the release an update or restart moves to adds over this build; null when either side published no totals. */
  changes: Changes | null;
  /** The newer version already on disk. Distinct from `latest`: a release can
   *  land between the install and the restart. */
  installedVersion: string | null;
  installedTag: string | null;
  restartRequired: boolean;
  /** Whether `restartApp` is honored; always true today, kept for a channel that cannot. */
  canRestart: boolean;
  /** Per-process id: changes when the server has relaunched. */
  instance: string;
}

export interface InstalledCli {
  link: string;
  /** The link's directory — what to add to PATH when `onPath` is false. */
  dir: string;
  target: string;
  onPath: boolean;
  alreadyCurrent: boolean;
}

// --- settings: kubernetes -----------------------------------------------------

export interface K8sPreflight {
  kubectlFound: boolean;
  reachable: boolean;
  canCreateJobs: boolean;
  canPatchJobs: boolean;
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

// --- settings: modal ----------------------------------------------------------

export type ModalTokenSource = "env" | "syncedEnv" | "modalToml";

export interface ModalSettings {
  tokenConfigured: boolean;
  tokenSource: ModalTokenSource | null;
  maskedTokenId: string | null;
  maskedTokenSecret: string | null;
  processEnv: boolean;
}

// --- settings: vercel ---------------------------------------------------------

export type VercelPlan = "hobby" | "pro" | "enterprise";

export interface VercelScopes {
  teams: { id: string; name: string; plan: VercelPlan | null }[];
  teamId: string;
  projects: { id: string; name: string }[];
}

export interface VercelSettings {
  configured: boolean;
  /** Credentials in orx's own process environment override anything saved here. */
  processEnv: boolean;
  oidc: boolean;
  maskedToken: string | null;
  projectId: string | null;
  scopes: VercelScopes | null;
  error: string | null;
}

// --- settings: env vars / git / harnesses ------------------------------------

export interface EnvVar {
  key: string;
  maskedValue: string;
  /** Set only for non-secret keys, which are shown and edited in the clear. */
  value?: string | null;
  inProcessEnv: boolean;
}

/** Where `source` says the resolved data dir came from. `env` means the
 * `$ORX_DATA_DIR` override forces it — the UI shows the field read-only. */
export type DataDirSource = "env" | "config" | "xdg" | "default";

export interface DataDirSettings {
  current: string;
  defaultPath: string;
  isDefault: boolean;
  source: DataDirSource;
}

export interface DataDirValidation {
  ok: boolean;
  error?: string;
  treeBytes?: number;
  freeBytes?: number;
  sameFilesystem?: boolean;
}

export interface SshHost {
  container?: string | null;
  host: string;
  hostname?: string;
  user?: string;
  port?: string;
  identityFile?: string;
  /** Most recent preflight result, persisted across restarts. */
  lastTest?: SshPreflight;
}

export interface SshSettings { hosts: SshHost[]; defaultHost: string | null }
export interface SshExecutionPreflight extends SshPreflight {
  container: { reference: string; ready: boolean; error: string | null } | null;
}

export interface SshConfigFile {
  path: string;
  content: string;
}

export type RemoteSessionStatus =
  | "connecting"
  | "needsInstall"
  | "needsUpdate"
  | "applying"
  | "connected"
  | "reconnecting"
  | "disconnected";

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
  /** The remote's version, or its canary tag; null when unknown. */
  buildId: string | null;
  dashboardProtocol: number | null;
  gatewayUrl: string;
  error: string | null;
  /** Server-derived repair command for a permission rejection, shown inline so
   * the user can copy it or run it in the terminal. */
  repairCommand: string | null;
  installPaths: RemoteInstallPaths | null;
  uiPreferences: { theme: string | null; locale: string | null };
  canStartNewHost: boolean;
}

/** `production` only for an official release built by the project's CI. */
export type BuildChannel = "production" | "development";

export type RuntimeInfo =
  | { kind: "local"; version: string; buildChannel: BuildChannel }
  | { kind: "ssh"; version: string; buildChannel: BuildChannel; dashboardProtocol: number; session: RemoteSessionInfo };

export interface RemoteStopPreview {
  instanceId: string;
  activeTurnCount: number;
  queuedMessageCount: number;
  pendingPermissionCount: number;
  activeRunCount: number;
  attachmentCount: number;
}

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
  /** The launch options below are absent from an older orx, which can't apply them. */
  qos?: string | null;
  timeLimit: string | null;
  /** `--cpus-per-task` default; null = the partition decides (often one core). */
  cpusPerTask: number | null;
  /** `--mem` default in Slurm syntax ("64G"); null = the partition decides. */
  mem: string | null;
  /** Extra `#SBATCH` arguments written into every job, after orx's own lines. */
  sbatchArgs?: string[];
  /** Whether every job carries `#SBATCH --ntasks=1`. */
  requestOneTask?: boolean;
  /** Login-node candidates, from ~/.ssh/config (same source as SSH). */
  hosts: SshHost[];
}

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

export interface RayPreflight {
  reachable: boolean;
  address: string;
  rayVersion: string | null;
  error: string | null;
}

export interface TrackioRuns {
  /** Null until something has logged to the local store. */
  dashboardUrl: string | null;
  project: string;
  /** Run ids and names that have logged to the project. */
  runs: string[];
  /** A private Space's read-only `__sign`; without it the browser gets a 404. */
  dashboardSign: string | null;
}

export interface TrackioPreflight {
  reachable: boolean;
  version: string | null;
  writeAccess: boolean | null;
  error: string | null;
  /** The Space's runtime stage, e.g. SLEEPING; null for a self-hosted server. */
  spaceStage: string | null;
}

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
  | "vercel"
  | "openresearch";

/** Cheap fs/env probe only — "worth trying", not "healthy". Deep health lives
 * in each backend's own settings endpoint, fetched when its setup surface opens. */
export interface ComputeTargetSummary {
  id: ComputeTargetId;
  configured: boolean;
  fromEnvironmentTab?: boolean;
  /**
   * The readiness check couldn't run (offline, unreadable ~/.ssh), so
   * `configured` is a guess rather than an answer. Absent for backends whose
   * state is decidable locally.
   */
  unverified?: boolean;
  /** OpenResearch only: the machine serving this workspace is signed in. */
  signedIn?: boolean;
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

export interface LocalGpu {
  name: string;
  memMib: number | null;
}

/** What `--backend local` runs on: this machine's detected hardware. */
export interface PublicNetwork {
  ip: string;
  isDatacenter: boolean | null;
}

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

export interface OpenResearchAccount {
  loggedIn: boolean;
  apiUrl: string | null;
  user: { id: string; email: string; firstName: string | null; lastName: string | null; alphaxivUrl: string | null } | null;
}

export class OpenResearchAccountError extends Error {
  constructor(public kind: "offline" | "server", public apiUrl: string, public status: number | null) { super(kind); }
}

export class AlphaXivNotConnectedError extends Error {}
export interface AlphaXivFolder { id: string; name: string; parentId: string | null; paperCount: number }
export interface AlphaXivPaper { paperGroupId: string; title: string; authors: string[]; authorCount: number; url: string }
export interface AlphaXivPage { papers: AlphaXivPaper[]; nextCursor: string | null }
export interface OpenResearchSettings {
  loggedIn: boolean;
  apiUrl: string | null;
  organizations?: { id: string; name: string; createdBy: string }[] | null;
  /**
   * Whether a registered key's private half is on THIS machine — `matched` is
   * the only state that can actually reach a box. Optional: an older `orx`
   * binary serving a newer ui omits it.
   */
  sshKeyStatus?: "matched" | "no_local_match" | "none_registered" | "unknown";
  /** The `.pub` on this machine worth registering; null if there isn't one. */
  sshKeyPath?: string | null;
  /** What "Add SSH key" would upload while no usable key is registered; a null fingerprint means it creates the key. */
  sshKeyToAdd?: { path: string; fingerprint: string | null } | null;
  error: string | null;
}

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

const isFilePresentation = (value: string | null): value is FilePresentation =>
  value === "image" || value === "audio" || value === "video" || value === "pdf" ||
  value === "text" || value === "unknown" || value === "download";

export interface ArtifactFileMetadata {
  size: number;
  presentation: FilePresentation;
}

export interface GitSettings {
  gitVersion: string | null;
  userName: string | null;
  userEmail: string | null;
  ghInstalled: boolean;
  githubAuthenticated: boolean;
}

/** Which literature sources discovery and paper reading may use (settings.json). */
export interface LitSourcesSettings {
  alphaxiv: boolean;
  openalex: boolean;
  biorxiv: boolean;
  pubmed: boolean;
}

export interface ProjectDefaultsSettings {
  githubForNewProjects: boolean;
  githubDefaultPromptSeen: boolean;
  ghInstalled: boolean;
  githubAuthenticated: boolean;
}

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

export type OnboardingStep = "welcome" | "environment" | "profile";
export type FirstActionSurface = "demo" | "project";
export type FirstAction =
  | "starter_click"
  | "typed_prompt"
  | "open_experiment"
  | "open_file"
  | "run_experiment"
  | "create_experiment"
  | "open_settings";

type UiEvent =
  | { name: "demo_welcome_choice"; choice: "explore_demo" | "create_project" | "dismiss" }
  | { name: "onboarding_step_viewed"; step: OnboardingStep }
  | { name: "demo_experiment_started"; kind: "curated" | "run"; experiment: string }
  | { name: "project_starter_clicked"; slot: number }
  | { name: "first_action"; surface: FirstActionSurface; action: FirstAction };

export type HarnessId =
  | "claude-code"
  | "codex"
  | "opencode"
  | "cursor"
  | "antigravity"
  | `acp:${string}`;

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
  /** The catalog's own human name ("Opus", "GPT-5.6 Sol"). */
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
  /** Whether the model accepts images; absent when the catalog doesn't say. */
  imageInput?: boolean;
  /** Its id names a free tier. */
  free?: boolean;
  parsed: ModelName;
}

/** What an offer adds to its model: Cursor's and Antigravity's effort, speed and thinking, Claude Code's long context. */
export interface ModelVariant {
  effort: string | null;
  fast: boolean;
  thinking: boolean;
  context: string | null;
}

/** A model's name as the server reads it, so the picker groups what Settings hides and delegation skips. */
export interface ModelName {
  /** The same for one model in every agent's catalog, `:free` added for its free tier. Settings turns models off by it. */
  key: string;
  /** `key` without its version or free tier: what a favorite stars. */
  family: string;
  /** `family` without tier words a lab reuses across versions: a newer version of any tier makes an older one old. */
  series: string;
  version: number[] | null;
  /** The catalog's name without variant words, dates or asides. */
  label: string;
  variant: ModelVariant;
  /** How far the catalog's name is from its model's plain variant; a tile runs the lowest by default. */
  variantRank: number;
}

/** Display label for a harness model: the catalog's own name when it has one,
 * else prettified from the id. */
export function harnessModelLabel(model: HarnessModel): string {
  if (!model.id.startsWith("orx-local-")) return model.displayName ? tidyModelName(model.displayName) : modelLabel(model.id);
  const provider = model.displayName?.split(" · ").slice(1).join(" · ").replace(/ \(local\)$/, "");
  const endpoint = provider === "OpenAI-compatible server" || provider === "Custom endpoint" || provider === "Custom Endpoint"
    ? m.local_models_custom_endpoint()
    : provider;
  const label = modelLabel(model.id.replace(/-(?:FP|BF)\d+$/i, ""));
  return endpoint ? `${label} · ${endpoint}` : label;
}

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

export type AgentIconColor = "gray" | "red" | "orange" | "amber" | "green" | "teal" | "blue" | "indigo" | "purple" | "pink";

/** An added agent's mark: its provider's logo, a glyph, an emoji or a small image, on its colour or in a plain outline. */
export type AgentIcon = { color?: AgentIconColor } & (
  | { brand: true }
  | { glyph: string }
  | { emoji: string }
  | { image: { type: "image/webp" | "image/png"; data: string } }
);

/** An account OpenResearch signs an agent in to, beside the user's own. */
export interface AgentProfile {
  id: string;
  name: string;
  icon?: AgentIcon;
}

export interface Harness {
  id: HarnessId;
  /** Set on the extra entry for each of OpenResearch's accounts; `id` stays the agent's. */
  profile?: AgentProfile;
  /** The name and icon the user gave an agent found on this computer; never on a profile's entry. */
  label?: { name?: string; icon?: AgentIcon };
  /** The user turned this found agent off in Settings; on OpenCode, only the providers it signed in to itself. */
  off?: boolean;
  name: string;
  installed: boolean;
  /** On PATH, but `--version` failed — a broken install a reinstall repairs.
   * Never `agentReady`: spawning it just dumps the CLI's own crash into chat. */
  installBroken: boolean;
  binPath?: string;
  /** `binPath` is OpenResearch's own copy, which it downloaded and keeps current. */
  managed?: boolean;
  version?: string;
  authenticated: boolean;
  authState: "ready" | "needsLogin" | "unknown" | "unsupported";
  authMethod?: "oauth" | "apiKey" | "thirdParty" | "local";
  authProvider?: string;
  loginEligible?: boolean;
  authCheckFailed?: boolean;
  accountLoading?: boolean;
  account?: string;
  accountName?: string;
  org?: string;
  plan?: string;
  agentReady: boolean;
  agentNote?: string;
  setupUrl?: string;
  authEnvKey?: string;
  needsConnectionAdapter?: boolean;
  adapterMissing?: boolean;
  logo?: string;
  /** Setup is blocked by something no install/update/login command repairs —
   * an environment credential overriding the saved login, a database the CLI
   * will not open. `agentNote` carries the repair; offer no setup button. */
  needsConfigRepair?: boolean;
  /** A running turn takes further input, so the composer steers instead of
   * queueing. Narrowed per installation (codex's legacy exec path can't). */
  supportsSteering: boolean;
  supportsCompaction: boolean;
  /** A snapshot answer whose model catalog is still filling in the
   * background — `models` stays empty until `harness.catalog`
   * arrives and a plain re-read swaps in the real list. */
  catalogPending?: boolean;
  models: HarnessModel[];
  /** The `models` id a new chat starts on, from the agent's own setting; set only when it's ready and lists a model. */
  defaultModel?: string;
  options: HarnessOptions;
}

export interface HarnessSetupCommands {
  /** Empty for an agent OpenResearch downloads itself (`managedFrom`). */
  install: string;
  /** The vendor bootstrap URL an install note quotes; `install` runs the
   * platform's own installer, which on Windows is a PowerShell script. */
  installUrl?: string;
  /** The host OpenResearch downloads this agent from, when it installs it itself;
   * absent from servers that predate it. */
  managedFrom?: string | null;
  login: string;
  update: string;
  requiresNpm: boolean;
}

export interface AcpDefinition {
  id: `acp:${string}`;
  name: string;
  executable: string;
  arguments: string[];
}
export interface AcpPreset extends AcpDefinition { setupUrl: string; needsConnectionAdapter: boolean }

/** Slash-skill offered in the composer's `/` dropdown; expanded server-side. */
export interface SkillInfo {
  name: string;
  disabled?: boolean;
  plugin?: string | null;
  harness?: string | null;
  description: string;
  /** Built-in composer commands share the menu with harness/user skills. */
  source?: "builtin" | "user" | "project" | "command";
}

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

/** A skill the agent gets in every session: a SKILL.md folder uploaded in the
 * Customize tab, or one mirrored from an installed coding agent or its plugins. */
export interface UserSkill {
  name: string;
  /** The coding agent or plugin it comes from; absent when uploaded here, which
   * is also the only case the user can edit or delete. */
  origin?: string | null;
  bytes: number;
  updatedAt: number;
}

export interface SkillDiscovery {
  agent: string;
  origin: string;
  path: string;
  status: "scanned" | "missing" | "unreadable";
  accepted: number;
  skipped: { path: string; reason: string }[];
}

const MODEL_WORDS: Record<string, string> = { gpt: "GPT", glm: "GLM", oss: "OSS", deepseek: "DeepSeek", minimax: "MiniMax", longcat: "LongCat", mimo: "MiMo" };

/** "openai/gpt-5.5" → "GPT-5.5", "anthropic/claude-opus-4-8" → "Opus 4.8", "deepseek-v4-flash" → "DeepSeek V4 Flash". */
export function modelLabel(id: string): string {
  const last = (id.split("/").pop() ?? id).replace(/^~/, "").replace(/^claude-/, "");
  const words: string[] = [];
  const nums: string[] = [];
  const version = () => {
    if (!nums.length) return;
    const number = nums.splice(0).join(".");
    // GPT and GLM write their version on: "GPT-5.5".
    if (words.at(-1) === "GPT" || words.at(-1) === "GLM") words.push(`${words.pop()}-${number}`);
    else words.push(number);
  };
  for (const part of last.split("-")) {
    if (/^\d+(\.\d+)?$/.test(part)) {
      nums.push(part);
    } else {
      version();
      words.push(MODEL_WORDS[part] ?? (/^\d+b$/.test(part) ? part.toUpperCase() : /^o\d+$/.test(part) ? part : part.charAt(0).toUpperCase() + part.slice(1)));
    }
  }
  version();
  return words.join(" ");
}

/** Evens out the names catalogs send: a lowercase id standing in for one, hyphens
 * between words ("DeepSeek-V4-Pro" → "DeepSeek V4 Pro", where "GPT-5.5" stays), lowercase
 * words outside parentheses, and doubled or zero-width spaces. A name the user gave a
 * second key, after " · ", stays as written. */
export function tidyModelName(name: string): string {
  const [model, ...owner] = name.replace(/[\u200B-\u200D\uFEFF]/g, "").split(" · ");
  const clean = model.replace(/\s+/g, " ").trim();
  if (!/[A-Z ]/.test(clean)) return [modelLabel(clean), ...owner].join(" · ");
  return [clean.split(/(\([^)]*\))/).map((part, index) => index % 2 ? part : part
    .replace(/(?<![A-Z]{2})-(?=[A-Z])|(?<=[a-z])-(?=\d)/g, " ")
    .replace(/(?<![\w-])(?!(?:and|for|the|via|with)\b)[a-z]{3,}\b/g, (word) => MODEL_WORDS[word] ?? word[0].toUpperCase() + word.slice(1))).join(""), ...owner].join(" · ");
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
  nativeChoices?: { id: string; label: string; kind: string }[];
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
  /** The chat whose agent answered for the user. */
  answeredBy?: string;
  /** Backend resume routing id. Presence marks a HELD mid-turn card (the
   * turn is blocked open waiting on this answer); absent on end-turn cards. */
  nativeId?: string;
}

export interface ChatPart {
  id: string;
  type: string; // text | reasoning | tool | prompt | image | steer
  phase?: "commentary" | "final_answer";
  text?: string;
  /** Original file name for an `image` (attachment) part, when known. */
  name?: string;
  tool?: string;
  state?: ChatToolState;
  prompt?: ChatPrompt;
  /** Nested transcript of a sub-agent this part spawned (Codex `subagent`
   * tool). Streams live and recurses for sub-agents that spawn their own. */
  children?: ChatPart[];
  /** Set on a message orx or another chat's agent wrote: what it is, and the chat or run it comes from. */
  orx?: OrxSource;
}

export interface OrxSource {
  kind: "task" | "message" | "answer" | "question" | "run" | "monitoring";
  from?: string;
  /** A run's status, or the state a chat's turn ended in. */
  status?: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  parts: ChatPart[];
  createdAt: number;
  completedAt?: number | null;
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
  awaitingInput?: boolean;
  id: string;
  projectId: string;
  harness: HarnessId;
  /** The account the chat runs on, when it's one OpenResearch keeps. */
  profile?: string | null;
  harnessName?: string | null;
  nativeConfiguration?: unknown;
  /** The models an ACP agent's live configuration offers, when it offers any. */
  nativeModels?: HarnessModel[] | null;
  title: string | null;
  /** Who wrote `title`: `"fallback"` (first-line placeholder), `"generated"`
   * (harness auto-title), `"user"` (explicitly chosen — a rename, or the title a
   * delegating agent gave). Null on legacy sessions. */
  titleSource?: string | null;
  model: string | null;
  serviceTier: string | null;
  permissionMode: string | null;
  /** Independent Plan axis for Codex/OpenCode/Cursor. */
  planMode: boolean;
  /** What `/goal` asked the agent to keep working toward; null when unset. */
  goal?: string | null;
  autonomy: Autonomy;
  reasoningLevel: string | null;
  /** Hidden from the default Recents list, but fully intact and resumable. */
  archived: boolean;
  /** Chat this side chat branched from. Side chats share its worktree and
   * stay out of top-level history; deleting the parent deletes its side chats. */
  sideParentSessionId?: string | null;
  /** Set on a chat another chat delegated to: who delegated, how deep it is, whether it tells
   * that chat when it finishes, how its latest turn ended, and what it waits on. */
  delegate?: {
    parentId: string;
    depth: number;
    notifies: boolean;
    state: string | null;
    answer: string | null;
    error: string | null;
    asking: "question" | "permission" | "plan" | null;
  } | null;
  createdAt: number;
  updatedAt: number;
  busy: boolean;
  activeLeafId: string | null;
  contextUsage?: ContextUsage;
}

/** Fields `PATCH /api/chat/sessions/<id>` changes; each is optional. */
export interface ChatSessionPatch {
  /** Trimmed by the server, which rejects an empty title. */
  title?: string;
  /** Archived chats stay resumable. */
  archived?: boolean;
  /** `false` keeps a delegate for the user: it stops telling the chat that started it and becomes a side chat. */
  notifyParent?: false;
  /** The session-specific Plan axis used by Codex, OpenCode and Cursor. */
  planMode?: boolean;
  /** `null` clears the goal. */
  goal?: string | null;
  autonomy?: Autonomy;
  agent?: AgentSelection;
  permissionMode?: string;
}

/** A chat the user had in an agent's own CLI, which orx has no session for. */
export interface NativeChat {
  harness: HarnessId;
  nativeId: string;
  title: string | null;
  cwd: string | null;
  updatedAt: number;
}

export type ChatSearchScope = "chat" | "project" | "all";
export interface ChatSearchParams {
  q: string;
  scope: ChatSearchScope;
  sessionId?: string;
  projectId?: string;
}
export interface ChatSearchHit {
  sessionId: string;
  projectId: string;
  messageId: string;
  partId: string | null;
  spawnPartId: string | null;
  role: string;
  createdAt: number;
  chatTitle: string | null;
  archived: boolean;
  onActiveBranch: boolean;
  snippet: string;
}
export interface ChatSearchPage {
  hits: ChatSearchHit[];
  cursor: string | null;
  indexing: boolean;
  requiresLongerQuery: boolean;
}
export interface SidebarChatCursor { updatedAt: number; id: string }
export interface SidebarChatPage { sessions: ChatSession[]; next: SidebarChatCursor | null }

/** Per-session (and per-turn) composer selections beyond the harness itself. */
export interface TurnOptions {
  model?: string | null;
  serviceTier?: string | null;
  permissionMode?: string | null;
  planMode?: boolean;
  reasoningLevel?: string | null;
}

/** A message the user sent while a turn was running, parked to run next. */
export interface QueuedMessage {
  id: string;
  text: string;
  planMode?: boolean;
  dispatchState?: "queued" | "retrying" | "blocked";
  nextRetryAt?: number | null;
  error?: string | null;
}

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

export interface ChatTurnResult {
  turnId: string;
  queued: boolean;
  existing: boolean;
}

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

// --- helpers shared across views --------------------------------------------

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
  if (typeof backend.namespace === "string" && backend.namespace) {
    const container = backend.sshContainer;
    if (container && typeof container === "object" && "reference" in container && typeof container.reference === "string") {
      return `${backend.namespace} / ${container.reference}`;
    }
    return backend.namespace;
  }
  return "";
}

export interface LocalModelConnection {
  id: string;
  name: string;
  icon?: AgentIcon;
  baseUrl: string;
  hasApiKey: boolean;
  models: Record<string, number>;
}

/** A provider OpenResearch keeps for OpenCode: a key, or a sign-in made through OpenCode. */
export interface OpenCodeProvider {
  /** OpenCode's provider id; a second key for one provider gets its own, naming the provider in `canonical`. */
  id: string;
  kind: "key" | "oauth";
  canonical?: string;
  /** The key's last characters, to tell keys apart. */
  keyEnding?: string;
  name: string;
  icon?: AgentIcon;
}

/** A provider OpenCode can sign in to, and how. */
export interface OpenCodeIntegration {
  id: string;
  name: string;
  methods: ({ type: "oauth"; id: string; label: string; form?: OpenCodeFormField[] } | { type: "key"; label?: string })[];
  connected: boolean;
}

export type OpenCodeAnswer = string | number | boolean;

/** A question OpenCode asks before a sign-in, shown only while every `when` holds. */
export interface OpenCodeFormField {
  key: string;
  type: string;
  title?: string;
  description?: string;
  placeholder?: string;
  required?: boolean;
  default?: OpenCodeAnswer;
  options?: { value: string; label: string; description?: string }[];
  when?: { key: string; op: "eq" | "neq"; value: OpenCodeAnswer }[];
}

/** `code` mode waits for a code the sign-in page shows; `auto` finishes on its own. */
export interface OpenCodeSignIn {
  attempt: string;
  url: string;
  mode: "auto" | "code";
  instructions: string;
}

export type CitedMessage = { projectId: string | null; sessionId: string; messageId: string; harness: HarnessId | null; preview: string };

export interface ChatActiveRun {
  sessionId: string;
  run: Run;
  experimentName: string | null;
}

export interface ShareChoice {
  chats: string[];
  logs: boolean;
  artifacts: boolean;
}

export interface OpenedSharedProject {
  project: Project;
  /** Experiment branches the file named but did not carry. */
  missingBranches: string[];
}

/** A published project link on openresearch.sh. */
export interface PublishedProject {
  id: string;
  slug: string;
  url: string;
  name: string;
  byteSize: number;
  publishedAt: string;
}

/** What anyone with a published link sees before opening it. */
export interface PublicProject {
  slug: string;
  url: string;
  name: string;
  byteSize: number;
  publisher: string | null;
}

/** Where a shared project comes from: a picked file, a file on this machine, or a published link. */
export type SharedSource = File | string | { link: string };

/** How work that orx up runs in the background is going: opening a shared project, or publishing one. */
export interface OperationProgress {
  phase: "downloading" | "unpacking" | "opening" | "preparing" | "uploading";
  bytes: number;
  totalBytes: number | null;
}

interface OperationStatus<T> extends OperationProgress {
  result: T | null;
  error: string | null;
}

export function createApi(
  client: WorkspaceClient,
  invalidation: WorkspaceInvalidation,
) {
  const {
    workspaceScope,
    isCurrentScope,
    replaceWorkspace,
    url,
    request: fetch,
  } = client;
  const { invalidateWrite, removeSession, removeProject } = invalidation;

  async function json<T>(res: Response): Promise<T> {
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      let message = text;
      try {
        const parsed: unknown = JSON.parse(text);
        if (typeof parsed === "object" && parsed !== null) {
          if ("error" in parsed && typeof parsed.error === "string") message = parsed.error;
          if (
            res.status === 409 &&
            "code" in parsed &&
            parsed.code === "fileChanged" &&
            "exists" in parsed &&
            typeof parsed.exists === "boolean"
          ) {
            const currentVersion =
              "currentVersion" in parsed && typeof parsed.currentVersion === "string"
                ? parsed.currentVersion
                : null;
            throw new FileChangedError(message, currentVersion, parsed.exists);
          }
          if (res.status === 409 && "code" in parsed && parsed.code === "opencode1") throw new OpenCode1Error(message);
          if (res.status === 400 && "code" in parsed && (parsed.code === "unreachable" || parsed.code === "needsKey" || parsed.code === "noModels"))
            throw new UnlistedError(message, parsed.code);
          if (res.status === 400 && "fields" in parsed && typeof parsed.fields === "object" && parsed.fields !== null)
            throw new FieldErrors(message, parsed.fields as Record<string, string[]>);
        }
      } catch (error) {
        if (error instanceof FileChangedError || error instanceof RejectedResponseError) throw error;
        // non-JSON body — show it raw
      }
      const ErrorType = res.status >= 400 && res.status < 500 ? RejectedResponseError : Error;
      throw new ErrorType(message || `HTTP ${res.status}`);
    }
    const data: T = await res.json();
    if (!isCurrentScope(workspaceScope()))
      throw new DOMException("Workspace changed", "AbortError");
    return data;
  }

  const get = <T>(url: string, signal?: AbortSignal) => fetch(url, { signal }).then((r) => json<T>(r));
  async function writeResponse(
    url: string,
    init: RequestInit,
  ): Promise<Response> {
    const scope = workspaceScope();
    const response = await fetch(url, init);
    if (!isCurrentScope(scope)) throw new DOMException("Workspace changed", "AbortError");
    if (response.ok) invalidateWrite(url, scope);
    return response;
  }
  const post = <T>(url: string, body?: unknown, keepalive = false) =>
    writeResponse(url, {
      method: "POST",
      keepalive,
      headers: body === undefined ? {} : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    }).then((r) => json<T>(r));
  const patch = <T>(url: string, body: unknown) =>
    writeResponse(url, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }).then((r) => json<T>(r));
  const put = <T>(url: string, body: unknown) =>
    writeResponse(url, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }).then((r) => json<T>(r));

  /** Byte-exact checkout file for browser-native media rendering or download. */
  const projectFileUrl = (
    projectId: string,
    path: string,
    opts: CheckoutRef = {},
  ) =>
    url(
      `/api/projects/${projectId}/file/raw?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
    );

  /** Byte-exact absolute-path file for browser-native media rendering or download. */
  const absoluteFileUrl = (path: string) =>
    url(`/api/files/abs/raw?path=${encodeURIComponent(path)}`);

  /** Page that posts the paper to Overleaf as a new project — the path for an
   * account whose plan has no Git integration. Opened in a tab, not fetched. */
  const overleafUploadUrl = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
  ) =>
    url(
      `/api/projects/${projectId}/file/overleaf/upload?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
    );

  /** The run on its Trackio dashboard, only once it has logged there. `embed` hides the dashboard chrome. */
  function trackioRunUrl(trackio: TrackioRuns | undefined, runId: string, embed?: { theme: string }) {
    if (!trackio?.dashboardUrl || !trackio.runs.includes(runId)) return null;
    const query = new URLSearchParams({ project: trackio.project, runs: runId });
    if (trackio.dashboardSign) query.set("__sign", trackio.dashboardSign);
    if (embed) {
      query.set("sidebar", "hidden");
      query.set("navbar", "hidden");
      query.set("__theme", embed.theme);
    }
    return `${trackio.dashboardUrl}/?${query}`;
  }

  /** Raw artifact bytes served by the compatibility `/files` API. */
  const artifactUrl = (projectId: string, path: string) =>
    url(
      `/api/projects/${projectId}/files/file?path=${encodeURIComponent(path)}`,
    );

  /** Image parts store a server-minted file name; this is where it's served. */
  const chatAttachmentUrl = (name: string) =>
    url(`/api/chat/attachments/${encodeURIComponent(name)}`);

  const listProjects = (signal?: AbortSignal) =>
    get<{ projects: Project[] }>("/api/projects", signal).then((r) => r.projects);

  const listProjectActivity = (signal?: AbortSignal) =>
    get<{ activity: ProjectActivity[] }>("/api/projects/activity", signal).then((r) => r.activity);

  const getUiState = (signal?: AbortSignal) => get<UiState>("/api/settings/ui-state", signal);

  const getProjectUiState = (projectId: string, signal?: AbortSignal) =>
    get<ProjectWorkspace | null>(`/api/projects/${encodeURIComponent(projectId)}/ui-state`, signal);

  const saveProjectUiState = (projectId: string, state: ProjectWorkspace, keepalive = false) =>
    post<ProjectWorkspace>(`/api/projects/${encodeURIComponent(projectId)}/ui-state`, state, keepalive);
  const saveGlobalWorkspace = (workspace: GlobalWorkspace, keepalive = false) =>
    post<UiState>("/api/settings/ui-state", { workspace }, keepalive);

  const updateUiState = (body: {
    workspace?: GlobalWorkspace;
    tourCompleted?: boolean;
    preferredAgent?: AgentSelection;
    preferredAutonomy?: Autonomy;
    nestedDelegation?: boolean;
    opencodeFreeModels?: boolean;
    favoriteModels?: string[];
    hiddenModels?: string[];
    hiddenAgents?: string[];
    modelSectionOrder?: string[];
    collapsedModelSections?: string[];
    modelSightings?: ModelSightings;
  }) => post<UiState>("/api/settings/ui-state", body);

  const completeOnboarding = (selection: OnboardingSelection) =>
    post<{ project: Project; selection: OnboardingSelection }>("/api/onboarding/complete", selection);

  const getProjectPathStatus = (path = "", signal?: AbortSignal) => {
    const query = path ? `?path=${encodeURIComponent(path)}` : "";
    return get<ProjectPathStatus>(`/api/project-path/status${query}`, signal);
  };

  const installGit = () => post<Record<string, never>>("/api/git/install");

  const pickProjectFolder = () =>
    post<{ path: string | null }>("/api/project-path/pick").then((result) => result.path);

  const createProject = (body: NewProject) =>
    post<CreateProjectResult>("/api/projects", body);

  const searchPapers = (q: string, signal?: AbortSignal) =>
    get<{ papers: PaperHit[] }>(`/api/papers/search?q=${encodeURIComponent(q)}`, signal).then(
      (r) => r.papers,
    );

  /** The signed-in GitHub login, for naming the account a new repo lands on.
   * `login` is null when there's no usable token. */
  const githubAccount = (signal?: AbortSignal) => get<{ login: string | null }>("/api/github/account", signal);
  const githubOrganizations = (signal?: AbortSignal) => get<{ organizations: string[] }>("/api/github/organizations", signal);

  const githubProjectRepoPreview = (name: string, signal?: AbortSignal) =>
    get<{ repo: string }>(`/api/github/project-repo-preview?name=${encodeURIComponent(name)}`, signal);

  /** Whether the stored credentials are explicitly confirmed to push to a repo. */
  const repoAccess = (owner: string, repo: string, signal?: AbortSignal) =>
    get<{ canPush: boolean }>(
      `/api/github/repo-access?owner=${encodeURIComponent(owner)}&repo=${encodeURIComponent(repo)}`,
      signal,
    );

  /** Resolve an arXiv or alphaXiv paper id / URL to title + linked GitHub repo. May take a few
   * seconds for papers alphaXiv hasn't indexed yet (it scrapes arXiv on a miss). */
  const resolvePaper = (id: string, signal?: AbortSignal) =>
    get<{ paper: ResolvedPaper }>(`/api/papers/resolve?id=${encodeURIComponent(id)}`, signal).then(
      (r) => r.paper,
    );

  const updateProject = (projectId: string, body: { runCommand?: string; name?: string }) =>
    patch<{ project: Project }>(`/api/projects/${projectId}`, body).then((r) => r.project);

  /** Start generating starter prompts for a project that is about to be created,
   *  so they are cached by the time its chat opens. Fire-and-forget. */
  const prewarmStarterPrompts = (body: {
    name: string;
    paperId?: string;
    path?: string;
    locale: string;
    starterAgent?: Pick<AgentSelection, "harness" | "model" | "profile">;
  }) => post<{ ok: boolean }>("/api/projects/starter-prompts/prewarm", body);

  /** Slow on a cache miss (one headless model call over a brief of the project). */
  const getProjectStarterPrompts = (
    projectId: string,
    harness: HarnessId,
    model: string | null,
    locale: string,
    profile: string | null,
    signal?: AbortSignal,
  ) =>
    get<ProjectStarterPrompts>(
      `/api/projects/${projectId}/starter-prompts?${new URLSearchParams({ harness, ...(model ? { model } : {}), ...(profile ? { profile } : {}), locale })}`,
      signal,
    );

  const deleteProject = (projectId: string, deleteArtifacts = false) =>
    writeResponse(`/api/projects/${projectId}?deleteArtifacts=${deleteArtifacts}`, { method: "DELETE" }).then(async (r) => {
      const body = await r.json().catch(() => null);
      if (!r.ok) throw new Error(body?.error ?? `delete failed (${r.status})`);
      removeProject(projectId);
      return typeof body?.artifactsError === "string" ? body.artifactsError : undefined;
    });

  const listExperiments = (projectId: string, signal?: AbortSignal) =>
    get<{ experiments: Experiment[] }>(`/api/projects/${projectId}/experiments`, signal).then(
      (r) => r.experiments,
    );

  const getSessionExperiments = (sessionId: string, signal?: AbortSignal) =>
    get<{ experiments: SessionExperiment[] }>(`/api/chat/sessions/${encodeURIComponent(sessionId)}/experiments`, signal)
      .then((r) => r.experiments);

  const deleteExperiment = (id: string) =>
    writeResponse(`/api/experiments/${id}`, { method: "DELETE" }).then((r) => json<{ ok: boolean; branchCommit: string | null }>(r));
  const setExperimentArchived = (id: string, direction: "ancestors" | "descendants" | "only" | "region" | "taskRegion", archived: boolean) =>
    patch<{ ids: string[] }>(`/api/experiments/${id}/archive`, { direction, archived });

  const listRuns = (projectId: string, signal?: AbortSignal) =>
    get<{ runs: Run[] }>(`/api/projects/${projectId}/runs`, signal).then((r) => r.runs);

  const listInstances = () =>
    get<{ instances: Instance[] }>("/api/instances").then((r) => r.instances);

  const cancelRun = (runId: string) =>
    post<{ ok: boolean }>(`/api/runs/${runId}/cancel`).then(() => undefined);

  const cleanupRun = (runId: string) =>
    post<"accepted" | "nothing_lingering">(`/api/runs/${runId}/cleanup`).then(() => undefined);

  const abandonRun = (runId: string) => post<Run>(`/api/runs/${runId}/abandon`);

  async function fetchLogPage(runId: string, start: number, end: number, ident: LogIdent | null, signal?: AbortSignal): Promise<LogPage> {
    const query = new URLSearchParams({ start: String(start), end: String(end) });
    if (ident !== null) query.set("ident", ident);
    const response = await fetch(`/api/runs/${runId}/log?${query}`, { signal });
    if (response.status === 503) {
      const body: unknown = await response.clone().json().catch(() => null);
      if (typeof body === "object" && body !== null && "since" in body && typeof body.since === "number" && "contact" in body && (body.contact === "unreachable" || body.contact === "needs_auth")) {
        const host = "host" in body && typeof body.host === "string" ? body.host : null;
        throw new LogPageUnavailable(host, body.since, body.contact);
      }
    }
    return json<LogPage>(response);
  }

  const listEngineHosts = (signal?: AbortSignal) => get<EngineHost[]>("/api/compute/hosts", signal);

  const retryEngineHost = (alias: string) => post<EngineHost>(`/api/compute/hosts/${encodeURIComponent(alias)}/retry`);

  const getRunDiff = (runId: string, signal?: AbortSignal) => get<DiffPayload>(`/api/runs/${runId}/diff`, signal);

  const getExperimentDiff = (experimentId: string, signal?: AbortSignal) =>
    get<DiffPayload>(`/api/experiments/${experimentId}/diff`, signal);

  /** One file from the project — a branch's committed copy when `ref` is given,
   * else a chat session's worktree, else the hub clone — capped server-side
   * (~512 KB). */
  const getProjectFile = (projectId: string, path: string, opts: CheckoutRef = {}, signal?: AbortSignal) =>
    get<ProjectFile>(
      `/api/projects/${projectId}/file?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
      signal,
    );

  /** One file by absolute path — the escape hatch for a file an agent references
   * that lives outside the checkout and artifacts. Server-side capped (~512 KB);
   * loopback-only, so it reads whatever the user running `orx up` can read. */
  const getAbsoluteFile = (path: string, signal?: AbortSignal) =>
    get<AbsoluteFile>(`/api/files/abs?path=${encodeURIComponent(path)}`, signal);

  /** Overwrite a text file in the project's live checkout (worktree when
   * `sessionId` is given, else the hub clone). Committed branch trees are
   * read-only, so pass no `ref`. */
  const saveProjectFile = (
    projectId: string,
    path: string,
    content: string,
    opts: { sessionId?: string; expectedVersion: string },
  ) =>
    put<{ ok: boolean; root: CheckoutRoot; bytesWritten: number; version: string }>(
      `/api/projects/${projectId}/file`,
      { path, content, sessionId: opts.sessionId, expectedVersion: opts.expectedVersion },
    );

  const manageProjectFile = (
    projectId: string,
    path: string,
    action: FileAction,
    opts: { sessionId?: string } = {},
  ) =>
    patch<FileActionResult>(`/api/projects/${projectId}/file`, {
      path,
      ...action,
      sessionId: opts.sessionId,
    });

  /** Open a checkout file on the machine running `orx up`, in the OS default app
   * for its type (the user's editor for source files). */
  const openFileInEditor = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
  ) =>
    post<{ ok: boolean }>(`/api/projects/${projectId}/file/open`, {
      path,
      sessionId: opts.sessionId,
    });

  const getFileManager = (locale: string, signal?: AbortSignal) =>
    get<{ os: string; name: string | null }>(`/api/file-manager?locale=${encodeURIComponent(locale)}`, signal);

  /** Reveal a checkout file in the OS file manager on the machine running `orx up`. */
  const revealFileInManager = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
  ) =>
    post<{ ok: boolean }>(`/api/projects/${projectId}/file/reveal`, {
      path,
      sessionId: opts.sessionId,
    });

  /** Whether the machine running `orx up` can compile LaTeX. */
  const getLatexEngine = (signal?: AbortSignal) => get<LatexEngine>("/api/latex/engine", signal);

  /** Compile a checkout `.tex` file to a PDF beside it, using whichever LaTeX
   * engine is installed on the machine running `orx up`. Rejects with a message
   * naming the install options when none is. */
  const compileLatex = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
  ) =>
    post<LatexCompileResult>(`/api/projects/${projectId}/file/latex`, {
      path,
      sessionId: opts.sessionId,
    });

  /** Whether a Git authentication token is stored — the machine-wide half of the
   * Overleaf state, for Settings. */
  const getOverleafSettings = (signal?: AbortSignal) =>
    get<OverleafSettings>("/api/overleaf/settings", signal);

  /** Store an Overleaf Git authentication token. Not validated here: only the Git
   * bridge can judge a token, and it needs a project to judge it against, so a
   * bad token surfaces from `linkOverleaf`. */
  const saveOverleafToken = (token: string) =>
    post<{ hasToken: boolean }>("/api/overleaf/token", { token });

  const deleteOverleafToken = () =>
    writeResponse("/api/overleaf/token", { method: "DELETE" }).then((r) => json<{ hasToken: boolean }>(r));

  /** Store the Overleaf browser session cookie the live channel signs in with.
   * Accepts the bare value or `name=value`; only a connection can say whether it
   * works, so a stale cookie surfaces from `startOverleafLive`. */
  const saveOverleafSession = (session: string, opts: { host?: string } = {}) =>
    post<{ hasSession: boolean }>("/api/overleaf/session", { session, host: opts.host });

  const deleteOverleafSession = () =>
    writeResponse("/api/overleaf/session", { method: "DELETE" }).then((r) =>
      json<{ hasSession: boolean }>(r),
    );

  /** Read the session cookie from a browser signed in to Overleaf on this
   * machine, so it need not be pasted. macOS only; reading Chrome/Edge/Brave/Arc
   * asks the Keychain for their cookie key. Rejects when no browser holds one. */
  const importOverleafSession = (opts: { host?: string } = {}) =>
    post<{ hasSession: boolean; source: string }>("/api/overleaf/session/import", { host: opts.host });

  /** Open the live channel, or keep it open: a tab calls this while it stays on
   * the file. One Overleaf refused stays refused unless `retry` is set. */
  const startOverleafLive = (
    projectId: string,
    path: string,
    opts: { sessionId?: string; retry?: boolean } = {},
  ) =>
    post<OverleafLive>(`/api/projects/${projectId}/file/overleaf/live`, {
      path,
      sessionId: opts.sessionId,
      retry: opts.retry,
    });

  const stopOverleafLive = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
  ) =>
    writeResponse(
      `/api/projects/${projectId}/file/overleaf/live?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
      {
        method: "DELETE",
      },
    ).then((r) => json<OverleafLive>(r));

  const getOverleafState = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
    signal?: AbortSignal,
  ) => get<OverleafState>(`/api/projects/${projectId}/file/overleaf?${checkoutQuery(opts, new URLSearchParams({ path }))}`, signal);

  /** Point this `.tex` at an Overleaf project. The server proves the account can
   * reach it before storing the link, so this is where a plan without Git
   * integration — or a bad token — is reported. */
  const linkOverleaf = (
    projectId: string,
    path: string,
    opts: { project: string; sessionId?: string },
  ) =>
    post<OverleafState>(`/api/projects/${projectId}/file/overleaf`, {
      path,
      project: opts.project,
      sessionId: opts.sessionId,
    });

  const unlinkOverleaf = (projectId: string, path: string, opts: { sessionId?: string } = {}) =>
    writeResponse(`/api/projects/${projectId}/file/overleaf?${checkoutQuery(opts, new URLSearchParams({ path }))}`, {
      method: "DELETE",
    }).then((r) => json<OverleafState>(r));

  /** Bring the paper and the linked Overleaf project into step, both ways. */
  const syncOverleaf = (
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
  const getOverleafStatus = (
    projectId: string,
    path: string,
    opts: { sessionId?: string } = {},
    signal?: AbortSignal,
  ) =>
    get<{ remoteChanged: boolean }>(
      `/api/projects/${projectId}/file/overleaf/status?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
      signal,
    );

  /** Flat file listing of the project — a branch's committed tree when `ref` is
   * given, else a chat session's live worktree when `sessionId` is given, else
   * the hub clone's checkout — plus the branch name. `ref` and `sessionId` are
   * mutually exclusive (the server rejects both). */
  const getCodeTree = (projectId: string, opts: CheckoutRef = {}, signal?: AbortSignal) => {
    const qs = checkoutQuery(opts).toString();
    return get<CodeTree>(`/api/projects/${projectId}/code-tree${qs ? `?${qs}` : ""}`, signal);
  };

  const getSessionWorktree = (sessionId: string, signal?: AbortSignal) =>
    get<SessionWorktree>(`/api/chat/sessions/${sessionId}/worktree`, signal);

  const getHfSettings = (signal?: AbortSignal) => get<HfSettings>("/api/settings/hf", signal);

  const saveHfToken = (token: string) => post<HfSettings>("/api/settings/hf", { token });

  const getTinkerSettings = (signal?: AbortSignal) => get<TinkerSettings>("/api/settings/tinker", signal);
  const saveTinkerKey = (key: string) => post<TinkerSettings>("/api/settings/tinker", { key });

  const getUpdateStatus = (signal?: AbortSignal) => get<UpdateStatus>("/api/update", signal);

  const getPendingWork = (signal?: AbortSignal) => get<PendingWork>("/api/pending-work", signal);

  const getBuildNotes = (signal?: AbortSignal) => get<BuildNotes | null>("/api/update/notes", signal);

  const applyUpdate = () => post<UpdateStatus>("/api/update/apply");

  /** Relaunch the server into the copy the updater installed, stopping any agent at work when `stopAgents`.
   *  The old server answers and then goes away; poll `getUpdateStatus` for the new one. */
  const restartApp = (stopAgents = false) =>
    post<{ restarting: boolean; version: string }>("/api/update/restart", { stopAgents });

  const setAutoUpdate = (enabled: boolean) =>
    post<UpdateStatus>("/api/update/auto", { enabled });

  const setReleaseChannel = (channel: ReleaseChannel) =>
    post<UpdateStatus>("/api/update/channel", { channel });

  const installCli = (force = false) =>
    post<InstalledCli>("/api/update/install-cli", { force });

  const getK8sSettings = (signal?: AbortSignal) => get<K8sSettings>("/api/settings/k8s", signal);

  const saveK8sSettings = (body: { context?: string; namespace?: string }) =>
    post<K8sSettings>("/api/settings/k8s", body);

  const getModalSettings = (signal?: AbortSignal) => get<ModalSettings>("/api/settings/modal", signal);
  const saveModalToken = (tokenId: string, tokenSecret: string) =>
    post<ModalSettings>("/api/settings/modal", { tokenId, tokenSecret });

  const getVercelSettings = (signal?: AbortSignal) => get<VercelSettings>("/api/settings/vercel", signal);
  /** Lists what a token can reach; `token` omitted uses the saved one. */
  const getVercelScopes = (token: string | null, teamId: string | null) =>
    post<VercelScopes>("/api/settings/vercel/scopes", { token, teamId });
  const saveVercelSetup = (setup: { token: string | null; teamId: string; projectId: string | null; newProject: string | null }) =>
    post<VercelSettings>("/api/settings/vercel", setup);

  const getEnvVars = (signal?: AbortSignal) =>
    get<{ vars: EnvVar[] }>("/api/settings/env", signal).then((r) => r.vars);

  const setEnvVar = (key: string, value: string) =>
    post<{ vars: EnvVar[] }>("/api/settings/env", { key, value }).then((r) => r.vars);

  const deleteEnvVar = (key: string) =>
    writeResponse(`/api/settings/env/${encodeURIComponent(key)}`, { method: "DELETE" })
      .then((r) => json<{ vars: EnvVar[] }>(r))
      .then((r) => r.vars);

  const getDataDir = (signal?: AbortSignal) => get<DataDirSettings>("/api/settings/data-dir", signal);

  const validateDataDir = (path: string) =>
    post<DataDirValidation>("/api/settings/data-dir/validate", { path });

  /** Set the path without moving (onboarding / already-populated target). */
  const setDataDir = (path: string) =>
    post<DataDirSettings>("/api/settings/data-dir", { path }).then((result) => { replaceWorkspace(); return result; });

  /** Kick off a relocate. Resolves once the move has *started* (HTTP 202); watch
   * `onDataDirMove` (events.ts) for `progress` / `done` / `error`. Throws on the
   * 409 in-flight guard with the server's message. */
  const moveDataDir = (path: string) =>
    post<{ started: boolean }>("/api/settings/data-dir/move", { path });
  const getSshSettings = (signal?: AbortSignal) => get<SshSettings>("/api/settings/ssh", signal);
  const saveSshHost = (body: { host: string; container: string | null }) =>
    post<{ ok: boolean }>("/api/settings/ssh", body);
  const saveSshDefault = (host: string | null) => post<{ ok: boolean }>("/api/settings/ssh/default", { host });
  const testSshExecution = (host: string, container: string | null) =>
    post<SshExecutionPreflight>("/api/settings/ssh/preflight", { host, container });

  const getSshConfig = (signal?: AbortSignal) => get<SshConfigFile>("/api/settings/ssh/config", signal);

  const saveSshConfig = (content: string, previousContent: string) =>
    put<{ ok: boolean }>("/api/settings/ssh/config", { content, previousContent });

  const getSshMasterStatus = (host: string, signal?: AbortSignal) =>
    get<{ running: boolean | null }>(`/api/settings/ssh/master?host=${encodeURIComponent(host)}`, signal);

  const getRuntime = (signal?: AbortSignal) => get<RuntimeInfo>("/_orx/runtime", signal);

  const listRemoteSessions = (signal?: AbortSignal) =>
    get<{ sessions: RemoteSessionInfo[] }>("/api/remote/sessions", signal).then((r) => r.sessions);

  const createRemoteSession = (
    host: string,
    uiPreferences: { theme: string | null; locale: string | null },
  ) => post<RemoteSessionInfo>("/api/remote/sessions", { host, uiPreferences });

  const reconnectRemoteSession = (id: string) =>
    post<RemoteSessionInfo>(`/api/remote/sessions/${encodeURIComponent(id)}/reconnect`);

  const disconnectRemoteSession = (id: string) =>
    post<RemoteSessionInfo>(`/api/remote/sessions/${encodeURIComponent(id)}/disconnect`);

  const installRemoteSession = (paths: RemoteInstallPaths) =>
    post<RemoteSessionInfo>("/_orx/install", paths);

  const reconnectCurrentRemote = () => post<RemoteSessionInfo>("/_orx/reconnect");

  const disconnectCurrentRemote = () => post<RemoteSessionInfo>("/_orx/disconnect");

  const startCurrentRemoteHost = () => post<RemoteSessionInfo>("/_orx/start-host");

  const getRemoteStopPreview = () => get<RemoteStopPreview>("/_orx/stop-host");

  const stopCurrentRemoteHost = (preview: RemoteStopPreview) =>
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

  const getSlurmSettings = (signal?: AbortSignal) => get<SlurmSettings>("/api/settings/slurm", signal);

  /** Empty string clears a field back to the cluster default. */
  const saveSlurmSettings = (body: {
    host?: string;
    partition?: string;
    account?: string;
    qos?: string;
    timeLimit?: string;
    /** 0 clears it back to the partition default. */
    cpusPerTask?: number;
    mem?: string;
    /** Replaces the saved list. */
    sbatchArgs?: string[];
    requestOneTask?: boolean;
  }) => post<SlurmSettings>("/api/settings/slurm", body);

  const getRaySettings = (signal?: AbortSignal) => get<RaySettings>("/api/settings/ray", signal);

  /** Empty string clears the saved address (fall back to env / default). */
  const saveRaySettings = (body: { address?: string }) =>
    post<RaySettings>("/api/settings/ray", body);

  /** Live-test a Ray Jobs / Dashboard endpoint. */
  const rayPreflight = (address?: string) =>
    post<RayPreflight>("/api/settings/ray/preflight", { address: address ?? null });

  const getTrackioRuns = (projectId: string, signal?: AbortSignal) =>
    get<TrackioRuns>(`/api/trackio/projects/${encodeURIComponent(projectId)}/runs`, signal);

  const trackioPreflight = () => post<TrackioPreflight>("/api/settings/trackio/preflight", {});

  const getComputeSettings = (projectId?: string, signal?: AbortSignal) =>
    get<ComputeSettings>(
      `/api/settings/compute${projectId ? `?projectId=${encodeURIComponent(projectId)}` : ""}`,
      signal,
    );

  /** Set (or clear, with backend: null) the default compute target. Responds
   * with the full compute payload so the caller reconciles in one shot. */
  const setComputeDefault = (body: {
    backend: ComputeTargetId | null;
    flavor?: string | null;
    projectId?: string;
  }) => post<ComputeSettings>("/api/settings/compute/default", body);

  const getPublicNetwork = (signal?: AbortSignal) => get<PublicNetwork>("/api/settings/network", signal);

  const getLocalMachine = (signal?: AbortSignal) => get<LocalMachine>("/api/settings/local", signal);

  const getOpenResearchAccount = async (signal?: AbortSignal) => {
    const result = await get<OpenResearchAccount | { failure: { kind: "offline" | "server"; apiUrl: string; status: number | null } }>("/api/account/openresearch", signal);
    if ("failure" in result) throw new OpenResearchAccountError(result.failure.kind, result.failure.apiUrl, result.failure.status);
    return result;
  };

  async function alphaXivGet<T>(url: string, signal?: AbortSignal): Promise<T> {
    const response = await fetch(url, { signal });
    if (response.status === 401) throw new AlphaXivNotConnectedError();
    return json<T>(response);
  }
  const getAlphaXivMe = (signal?: AbortSignal) => alphaXivGet<{ avatarUrl: string | null }>("/api/alphaxiv/me", signal);
  const getAlphaXivFolders = (signal?: AbortSignal) => alphaXivGet<AlphaXivFolder[]>("/api/alphaxiv/folders", signal);
  const getAlphaXivPapers = (folder: string | null, cursor: string | null, search?: string, signal?: AbortSignal) => {
    const params = new URLSearchParams();
    if (folder) params.set("folder", folder);
    if (cursor) params.set("cursor", cursor);
    if (search) params.set("search", search);
    return alphaXivGet<AlphaXivPage>(`/api/alphaxiv/papers?${params}`, signal);
  };

  const logoutOpenResearch = () => post<{ ok: boolean }>("/api/settings/openresearch/logout");
  const addOpenResearchSshKey = () => post<{ ok: boolean }>("/api/settings/openresearch/ssh-key");

  const getOpenResearchSettings = (signal?: AbortSignal) =>
    get<OpenResearchSettings>("/api/settings/openresearch", signal);

  const getArtifacts = (projectId: string, signal?: AbortSignal) =>
    get<ProjectArtifacts>(`/api/projects/${projectId}/files`, signal);

  /** Delete a file or folder in the artifacts directory. */
  const deleteArtifact = (projectId: string, path: string) =>
    writeResponse(`/api/projects/${projectId}/files?path=${encodeURIComponent(path)}`, {
      method: "DELETE",
    }).then((r) => json<{ ok: boolean }>(r));

  const manageArtifactFile = (projectId: string, path: string, action: FileAction) =>
    patch<FileActionResult>(`/api/projects/${projectId}/files`, { path, ...action });

  /** Reveal an artifact file or folder in the OS file manager on the machine
   * running `orx up`. */
  const revealArtifactInManager = (projectId: string, path: string) =>
    post<{ ok: boolean }>(`/api/projects/${projectId}/files/reveal`, { path });

  /** Text-safe body of an artifact, or `null` when the file is missing. */
  const getArtifactFileText = (
    projectId: string,
    path: string,
    signal?: AbortSignal,
  ): Promise<FileTextBody | null> => {
    return fetch(artifactUrl(projectId, path), { signal,
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

  /** Lightweight existence/type probe used before an artifact preview. */
  const getArtifactFileMetadata = (
    projectId: string,
    path: string,
    signal?: AbortSignal,
  ): Promise<ArtifactFileMetadata | null> =>
    fetch(artifactUrl(projectId, path), { signal, method: "HEAD" }).then((r) => {
      if (r.status === 404) return null;
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      const presentation = r.headers.get("x-openresearch-presentation");
      return {
        size: Number(r.headers.get("content-length")) || 0,
        presentation: isFilePresentation(presentation) ? presentation : "download",
      };
    });

  const getGitSettings = () => get<GitSettings>("/api/settings/git");

  const saveGitSettings = (body: { userName?: string; userEmail?: string }) =>
    post<GitSettings>("/api/settings/git", body);

  const getLitSources = (signal?: AbortSignal) =>
    get<LitSourcesSettings>("/api/settings/lit-sources", signal);

  const setLitSources = (body: LitSourcesSettings) =>
    post<LitSourcesSettings>("/api/settings/lit-sources", body);

  const getProjectDefaults = (signal?: AbortSignal) =>
    get<ProjectDefaultsSettings>("/api/settings/projects", signal);

  const setProjectDefaults = (
    githubForNewProjects: boolean,
    githubDefaultPromptSeen?: boolean,
  ) =>
    post<ProjectDefaultsSettings>("/api/settings/projects", {
      githubForNewProjects,
      ...(githubDefaultPromptSeen === undefined ? {} : { githubDefaultPromptSeen }),
    });

  const getProjectGitStatus = (projectId: string, signal?: AbortSignal) =>
    get<ProjectGitStatus>(`/api/projects/${projectId}/git`, signal);

  const initializeProjectGit = (projectId: string) =>
    post<ProjectGitStatus>(`/api/projects/${projectId}/git/init`);

  const enableProjectGithub = (projectId: string, organization?: string) =>
    post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github`, {
      ...(organization ? { organization } : {}),
    });

  const disableProjectGithub = (projectId: string) =>
    post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github/disable`);

  const pushProjectGithub = (projectId: string) =>
    post<{ project: Project; git: ProjectGitStatus }>(`/api/projects/${projectId}/github/push`);

  /** The project as a `.orx` file, and the archive paths left out for size. */
  async function shareProject(projectId: string, options: ShareChoice) {
    const res = await writeResponse(`/api/projects/${projectId}/share`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(options),
    });
    if (!res.ok) return json<never>(res);
    const header = res.headers.get("x-orx-share");
    const summary: { skipped: string[] } = header ? JSON.parse(decodeURIComponent(header)) : { skipped: [] };
    const fileName = decodeURIComponent(/filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1] ?? "project.orx");
    return { file: await res.blob(), fileName, skipped: summary.skipped };
  }

  /** Whether the machine running the project can publish, and the project's link if it has one. */
  const getProjectLink = (projectId: string, signal?: AbortSignal) =>
    get<{ signedIn: boolean; sharedProject: PublishedProject | null }>(`/api/projects/${projectId}/publish`, signal);

  /** Follows background work `started` until it finishes; an older orx up answers with the result itself. */
  async function followOperation<T extends object>(started: T | { operationId: string }, invalidates: string, onProgress?: (progress: OperationProgress) => void): Promise<T> {
    if (!("operationId" in started)) return started;
    const scope = workspaceScope();
    const path = `/api/operations/${encodeURIComponent(started.operationId)}`;
    let missed = 0;
    for (;;) {
      let status: OperationStatus<T>;
      try {
        status = await get<OperationStatus<T>>(path);
        missed = 0;
      } catch (error) {
        // The work carries on in orx up, so a poll lost to the connection, even mid-body, is worth asking again.
        if (!(error instanceof ConnectionLostError || error instanceof TypeError)) throw error;
        if (++missed > 3) throw error instanceof TypeError ? new ConnectionLostError(m.request_connection_lost(), { cause: error }) : error;
        await new Promise((resolve) => setTimeout(resolve, 1000 * missed));
        continue;
      }
      if (status.error !== null) throw new Error(status.error);
      if (status.result !== null) {
        invalidateWrite(invalidates, scope);
        return status.result;
      }
      onProgress?.(status);
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
  }

  /** Upload the project to openresearch.sh; publishing again updates the same link. */
  const publishProject = async (projectId: string, options: ShareChoice, onProgress?: (progress: OperationProgress) => void) => {
    const url = `/api/projects/${projectId}/publish`;
    type Published = { sharedProject: PublishedProject; skipped: string[] };
    return followOperation(await post<Published | { operationId: string }>(`${url}?background=true`, options), url, onProgress);
  };

  /** What anyone with a published link sees, fetched by the machine that will open it. */
  const getPublicProject = (slug: string, signal?: AbortSignal) =>
    get<{ sharedProject: PublicProject }>(`/api/shared-projects/${encodeURIComponent(slug)}`, signal);

  /** Open a shared project as a new project in `path`, following it until it opens. */
  const importSharedProject = async (path: string, source: SharedSource, onProgress?: (progress: OperationProgress) => void) => {
    const url = `/api/projects/import?path=${encodeURIComponent(path)}&background=true`;
    type Started = OpenedSharedProject | { operationId: string };
    const start = () => {
      if (typeof source === "string") return post<Started>(`${url}&file=${encodeURIComponent(source)}`);
      if (!(source instanceof File)) return post<Started>(`${url}&shared=${encodeURIComponent(source.link)}`);
      return writeResponse(url, {
        method: "POST",
        headers: { "content-type": "application/octet-stream" },
        body: source,
      }).then((r) => json<Started>(r));
    };
    return followOperation(await start(), "/api/projects/import", onProgress);
  };

  const getTelemetry = (signal?: AbortSignal) => get<TelemetrySettings>("/api/settings/telemetry", signal);

  const setTelemetry = (enabled: boolean) =>
    post<TelemetrySettings>("/api/settings/telemetry", { enabled });

  /** Product events raised by the UI. Fire-and-forget: analytics must never
   * surface an error or block the interaction that triggered it. */
  const captureUiEvent = (event: UiEvent): void => {
    void post<{ ok: boolean }>("/api/telemetry/event", event).catch(() => {});
  };

  const reportLocale = (locale: string, signal?: AbortSignal) =>
    writeResponse("/api/telemetry/locale", {
      method: "POST",
      signal,
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ locale }),
    }).then((r) => json<{ locale: string }>(r));

  const getHarnessSetupCommands = (signal?: AbortSignal) =>
    get<Record<HarnessId, HarnessSetupCommands>>("/api/harnesses/setup/commands", signal);

  const getHarnesses = (refresh = false, retryRejected = false, signal?: AbortSignal, harness?: HarnessId) => {
    const params = new URLSearchParams();
    if (refresh) params.set("refresh", "1");
    if (retryRejected) params.set("retry", "1");
    if (harness) params.set("harness", harness);
    const query = params.size > 0 ? `?${params.toString()}` : "";
    return get<{ harnesses: Harness[] }>(`/api/harnesses${query}`, signal).then((r) => r.harnesses);
  };

  const listAcpHarnesses = () => get<AcpDefinition[]>("/api/harnesses/acp");
  const listAcpPresets = () => get<AcpPreset[]>("/api/harnesses/acp/presets");
  const saveAcpHarness = (definition: Omit<AcpDefinition, "id">, id?: string) => id
    ? put<AcpDefinition>(`/api/harnesses/acp/${encodeURIComponent(id)}`, definition)
    : post<AcpDefinition>("/api/harnesses/acp", definition);
  const removeAcpHarness = (id: string) => writeResponse(`/api/harnesses/acp/${encodeURIComponent(id)}`, { method: "DELETE" }).then((response) => json<{ ok: boolean }>(response));
  /** Deletes OpenResearch's copy of an agent, and what it kept only for that copy. */
  const removeHarnessInstall = (id: HarnessId) => writeResponse(`/api/harnesses/${encodeURIComponent(id)}/install`, { method: "DELETE" }).then(async (response) => {
    if (!response.ok) await json(response);
  });
  const testAcpHarness = (definition: Omit<AcpDefinition, "id">) => post<{ status: string; agentInfo?: { version?: string }; harnessId: AcpDefinition["id"] | null }>("/api/harnesses/acp/test", definition);
  const setChatConfiguration = (sessionId: string, optionId: string, value: string) => post<ChatSession>(`/api/chat/sessions/${sessionId}/configuration`, { optionId, value });

  const getSkills = (signal?: AbortSignal, harness?: string, projectId?: string) => {
    const query = new URLSearchParams({ ...(harness ? { harness } : {}), ...(projectId ? { project: projectId } : {}) });
    return get<{ skills: SkillInfo[] }>(`/api/skills${query.size ? `?${query}` : ""}`, signal);
  };

  const getSkillContent = (name: string, projectId?: string, signal?: AbortSignal, harness?: string | null) =>
    get<{ content: string }>(
      `/api/skills/${encodeURIComponent(name)}${
        `?${new URLSearchParams({ ...(projectId ? { project: projectId } : {}), ...(harness ? { harness } : {}) })}`
      }`,
      signal,
    ).then((r) => r.content);

  const listLatexTemplates = (signal?: AbortSignal) =>
    get<{ templates: LatexTemplate[] }>("/api/latex-templates", signal).then((r) => r.templates);

  const uploadLatexTemplate = (body: { filename: string; contentBase64: string }) =>
    post<{ template: LatexTemplate }>("/api/latex-templates", body).then((r) => r.template);

  const deleteLatexTemplate = (name: string) =>
    writeResponse(`/api/latex-templates?name=${encodeURIComponent(name)}`, { method: "DELETE" }).then((r) =>
      json<{ ok: boolean }>(r),
    );

  const listUserSkills = (signal?: AbortSignal) =>
    get<{ skills: UserSkill[]; discovery?: SkillDiscovery[] }>("/api/user-skills", signal);

  /** Upload a SKILL.md file or a .zip of a skill folder. `contentBase64` is the
   * raw file bytes; `filename`'s extension selects single-file vs archive. */
  const uploadUserSkill = (req: { filename: string; contentBase64: string }) =>
    post<{ skill: UserSkill }>("/api/user-skills", req).then((r) => r.skill);

  const deleteUserSkill = (name: string) =>
    writeResponse(`/api/user-skills?name=${encodeURIComponent(name)}`, { method: "DELETE" }).then((r) =>
      json<{ ok: boolean }>(r),
    );

  /** The full `SKILL.md`, frontmatter included, of an uploaded skill. */
  const getUserSkillContent = (name: string) =>
    get<{ name: string; content: string }>(
      `/api/user-skills/${encodeURIComponent(name)}`,
    ).then((r) => r.content);

  const updateUserSkill = (name: string, content: string) =>
    put<{ skill: UserSkill }>(
      `/api/user-skills/${encodeURIComponent(name)}`,
      { content },
    ).then((r) => r.skill);

  const listChatSessions = (projectId: string, signal?: AbortSignal) =>
    get<{ sessions: ChatSession[] }>(
      `/api/chat/sessions?projectId=${encodeURIComponent(projectId)}`,
      signal,
    ).then((r) => r.sessions);

  const listNativeChats = (signal?: AbortSignal) =>
    get<{ chats: NativeChat[] }>("/api/chat/native-sessions", signal).then((r) => r.chats);

  /** Adopt one, as a session that resumes the agent's own chat. */
  const importNativeChat = (projectId: string, chat: NativeChat) =>
    post<{ session: ChatSession }>("/api/chat/native-sessions/import", {
      projectId,
      harness: chat.harness,
      nativeId: chat.nativeId,
      title: chat.title,
    }).then((r) => r.session);

  /** Every project's sessions, newest first, for the composer's `/resume` picker. */
  const listAllChatSessions = (signal?: AbortSignal) =>
    get<{ sessions: ChatSession[] }>("/api/chat/sessions?scope=all", signal).then((r) => r.sessions);

  const searchChatMessages = (query: ChatSearchParams, cursor: string | null, signal?: AbortSignal) => {
    const params = new URLSearchParams({ q: query.q, scope: query.scope });
    if (query.sessionId) params.set("sessionId", query.sessionId);
    if (query.projectId) params.set("projectId", query.projectId);
    if (cursor) params.set("cursor", cursor);
    return get<ChatSearchPage>(`/api/search?${params}`, signal);
  };

  const listSidebarChatSessions = (filter: "active" | "archived" | "all", before: SidebarChatCursor | null, signal?: AbortSignal) => {
    const params = new URLSearchParams({ scope: "sidebar" });
    if (filter !== "all") params.set("archived", String(filter === "archived"));
    if (before) { params.set("beforeUpdatedAt", String(before.updatedAt)); params.set("beforeId", before.id); }
    return get<SidebarChatPage>(`/api/chat/sessions?${params}`, signal);
  };

  const createChatSession = (
    projectId: string,
    harness: HarnessId,
    opts: TurnOptions & { autonomy?: Autonomy; profile?: string | null } = {},
  ) =>
    post<{ session: ChatSession }>("/api/chat/sessions", { projectId, harness, ...opts }).then(
      (r) => r.session,
    );

  const getChatSession = (sessionId: string) =>
    get<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`).then((r) => r.session);

  const deleteChatSession = (sessionId: string) =>
    writeResponse(`/api/chat/sessions/${sessionId}`, { method: "DELETE" }).then((r) =>
      json<{ ok: boolean }>(r).then((result) => { removeSession(sessionId); return result; }),
    );

  const patchChatSession = (sessionId: string, changes: ChatSessionPatch) =>
    patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, changes).then(
      (r) => r.session,
    );

  /** The message an agent cited by ref: where it lives, the agent that wrote it if one did, and its first words. */
  const citedMessage = (cited: string) => get<CitedMessage>(`/api/chat/messages/${encodeURIComponent(cited)}`);

  /** Branch a persistent side chat off a snapshot of `sessionId`'s transcript. */
  const openSideChat = (sessionId: string) =>
    post<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}/side`, {}).then(
      (r) => r.session,
    );

  const getChatMessages = (sessionId: string, signal?: AbortSignal) =>
    get<{ messages: ChatMessage[]; queued?: QueuedMessage[]; activeLeafId?: string | null }>(
      `/api/chat/sessions/${sessionId}/messages`,
      signal,
    ).then((r) => ({
      messages: r.messages,
      queued: r.queued ?? [],
      activeLeafId: r.activeLeafId ?? null,
    }));

  /** Remove a still-parked message. */
  const cancelQueuedMessage = (sessionId: string, itemId: string) =>
    writeResponse(`/api/chat/sessions/${sessionId}/queue/${encodeURIComponent(itemId)}`, {
      method: "DELETE",
    }).then((r) => json<{ ok: boolean; removed: boolean }>(r));

  /** Retry the same parked message after safe queue delivery was exhausted. */
  const retryQueuedMessage = (sessionId: string, itemId: string) =>
    post<{ ok: boolean; retried: boolean }>(
      `/api/chat/sessions/${sessionId}/queue/${encodeURIComponent(itemId)}`,
    );

  /** Returns immediately; the turn streams over /api/events (chat.* events). */
  const sendChatMessage = (
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

  /** A positive result confirms admission; absence does not prove rejection. */
  const getChatSubmission = (sessionId: string, clientTurnId: string) =>
    get<{ accepted: boolean }>(
      `/api/chat/sessions/${encodeURIComponent(sessionId)}/submissions/${encodeURIComponent(clientTurnId)}`,
    );

  /** Compact a chat's context: natively where the agent can, else by summarizing. */
  const compactChatSession = (sessionId: string) =>
    post<{ message: ChatMessage }>(`/api/chat/sessions/${sessionId}/compact`, {});

  /** A composer `!` command, run in the session's checkout and recorded on its
   * transcript as a user-side exchange the next turn is told about. */
  const runShellCommand = (sessionId: string, command: string) =>
    post<{ message: ChatMessage }>(`/api/chat/sessions/${sessionId}/shell`, { command });

  const recoverChatTurn = (
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
  const forkChatTurn = (sessionId: string, messageId: string, text?: string) =>
    post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/fork`, { messageId, text });

  /** Show a different fork of a turn, along with the whole branch under it. */
  const selectChatBranch = (sessionId: string, leafId: string) =>
    post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/branch`, { leafId });

  const interruptChat = (sessionId: string) =>
    post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/interrupt`);

  const respondChat = (sessionId: string, answer: PromptAnswer) =>
    post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/respond`, answer);
  const getLocalModels = (signal?: AbortSignal) => get<LocalModelConnection[]>("/api/local-models", signal);
  const discoverLocalModels = (request: { baseUrl: string; apiKey: string }) => post<{ models: string[] }>("/api/local-models/discover", request);
  const connectLocalModel = (request: { name: string; baseUrl: string; apiKey: string; model: string; contextWindow: number }) => post<{ model: string; created: boolean }>("/api/local-models", request);
  const checkLocalModel = (id: string) => post<{ models: string[] }>(`/api/local-models/${encodeURIComponent(id)}/check`, {});
  /** Removes one model, or with none named the whole connection. */
  const removeLocalModel = (id: string, model?: string) => writeResponse(`/api/local-models/${encodeURIComponent(id)}${model ? `?${new URLSearchParams({ model })}` : ""}`, { method: "DELETE" }).then((r) => json<{ ok: boolean }>(r));

  const createAgentProfile = (body: { harness: "claude-code" | "codex"; name: string; icon?: AgentIcon }) => post<{ id: string }>("/api/agent-profiles", body);
  /**
   * Names a row of Settings → Agents, keyed as the picker keys it: an agent's or account's id, or `opencode:` and a
   * provider's or endpoint's id. `null` clears the icon, and a found agent's name back to its own.
   */
  const updateAgent = (key: string, body: { name?: string | null; icon?: AgentIcon | null }) => patch<unknown>(`/api/agents/${encodeURIComponent(key)}`, body);
  const removeAgentProfile = (id: string) => writeResponse(`/api/agent-profiles/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => json<unknown>(r));

  const getOpenCodeProviders = (signal?: AbortSignal) => get<OpenCodeProvider[]>("/api/opencode/providers", signal);
  /** A second key for a provider comes back under an id of its own. */
  const addOpenCodeProvider = (body: { provider: string; apiKey: string; name: string; icon?: AgentIcon }) => post<{ id: string }>("/api/opencode/providers", body);
  const replaceOpenCodeProviderKey = (id: string, apiKey: string) => put<unknown>(`/api/opencode/providers/${encodeURIComponent(id)}/key`, { apiKey });
  const removeOpenCodeProvider = (id: string) => writeResponse(`/api/opencode/providers/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => json<unknown>(r));
  const getOpenCodeIntegrations = (signal?: AbortSignal) => get<OpenCodeIntegration[]>("/api/opencode/integrations", signal);
  const startOpenCodeSignIn = (body: { integration: string; method: string; name: string; icon?: AgentIcon; answer?: Record<string, OpenCodeAnswer> }) => post<OpenCodeSignIn>("/api/opencode/sign-in", body);
  const getOpenCodeSignIn = (attempt: string, signal?: AbortSignal) =>
    get<{ status: "pending" | "complete" | "failed" | "expired"; message?: string }>(`/api/opencode/sign-in/${encodeURIComponent(attempt)}`, signal);
  const sendOpenCodeSignInCode = (attempt: string, code: string) => post<unknown>(`/api/opencode/sign-in/${encodeURIComponent(attempt)}/code`, { code });
  const cancelOpenCodeSignIn = (attempt: string) => writeResponse(`/api/opencode/sign-in/${encodeURIComponent(attempt)}`, { method: "DELETE" }).then((r) => json<unknown>(r));

  const listChatActiveRuns = (signal?: AbortSignal) =>
    get<ChatActiveRun[]>("/api/chat/active-runs", signal);

  const getSubagentTranscript = (sessionId: string, partId: string, signal?: AbortSignal) =>
    get<ChatPart | null>(`/api/chat/sessions/${sessionId}/subagent?partId=${encodeURIComponent(partId)}`, signal);

  return {
    projectFileUrl,
    absoluteFileUrl,
    overleafUploadUrl,
    trackioRunUrl,
    artifactUrl,
    chatAttachmentUrl,
    listProjects,
    listProjectActivity,
    getUiState,
    getProjectUiState,
    saveProjectUiState,
    saveGlobalWorkspace,
    updateUiState,
    completeOnboarding,
    getProjectPathStatus,
    installGit,
    pickProjectFolder,
    createProject,
    searchPapers,
    githubAccount,
    githubOrganizations,
    githubProjectRepoPreview,
    repoAccess,
    resolvePaper,
    updateProject,
    prewarmStarterPrompts,
    getProjectStarterPrompts,
    deleteProject,
    listExperiments,
    getSessionExperiments,
    deleteExperiment,
    setExperimentArchived,
    listRuns,
    listInstances,
    cancelRun,
    cleanupRun,
    abandonRun,
    fetchLogPage,
    listEngineHosts,
    retryEngineHost,
    getRunDiff,
    getExperimentDiff,
    getProjectFile,
    getAbsoluteFile,
    saveProjectFile,
    manageProjectFile,
    openFileInEditor,
    getFileManager,
    revealFileInManager,
    getLatexEngine,
    compileLatex,
    getOverleafSettings,
    saveOverleafToken,
    deleteOverleafToken,
    saveOverleafSession,
    deleteOverleafSession,
    importOverleafSession,
    startOverleafLive,
    stopOverleafLive,
    getOverleafState,
    linkOverleaf,
    unlinkOverleaf,
    syncOverleaf,
    getOverleafStatus,
    getCodeTree,
    getSessionWorktree,
    getHfSettings,
    saveHfToken,
    getTinkerSettings,
    saveTinkerKey,
    getUpdateStatus,
    getPendingWork,
    getBuildNotes,
    applyUpdate,
    restartApp,
    setAutoUpdate,
    setReleaseChannel,
    installCli,
    getK8sSettings,
    saveK8sSettings,
    getModalSettings,
    saveModalToken,
    getVercelSettings,
    getVercelScopes,
    saveVercelSetup,
    getEnvVars,
    setEnvVar,
    deleteEnvVar,
    getDataDir,
    validateDataDir,
    setDataDir,
    moveDataDir,
    getSshSettings,
    saveSshHost,
    saveSshDefault,
    testSshExecution,
    getSshConfig,
    saveSshConfig,
    getSshMasterStatus,
    getRuntime,
    listRemoteSessions,
    createRemoteSession,
    reconnectRemoteSession,
    disconnectRemoteSession,
    installRemoteSession,
    reconnectCurrentRemote,
    disconnectCurrentRemote,
    startCurrentRemoteHost,
    getRemoteStopPreview,
    stopCurrentRemoteHost,
    getSlurmSettings,
    saveSlurmSettings,
    getRaySettings,
    saveRaySettings,
    rayPreflight,
    getTrackioRuns,
    trackioPreflight,
    getComputeSettings,
    setComputeDefault,
    getLocalMachine,
    getPublicNetwork,
    getOpenResearchSettings,
    getOpenResearchAccount,
    getAlphaXivMe,
    getAlphaXivFolders,
    getAlphaXivPapers,
    logoutOpenResearch,
    addOpenResearchSshKey,
    getArtifacts,
    deleteArtifact,
    manageArtifactFile,
    revealArtifactInManager,
    getArtifactFileText,
    getArtifactFileMetadata,
    getGitSettings,
    saveGitSettings,
    getLitSources,
    setLitSources,
    getProjectDefaults,
    setProjectDefaults,
    getProjectGitStatus,
    initializeProjectGit,
    enableProjectGithub,
    disableProjectGithub,
    pushProjectGithub,
    shareProject,
    getProjectLink,
    publishProject,
    getPublicProject,
    importSharedProject,
    getTelemetry,
    setTelemetry,
    captureUiEvent,
    reportLocale,
    getHarnessSetupCommands,
    getHarnesses,
    listAcpHarnesses,
    listAcpPresets,
    saveAcpHarness,
    removeAcpHarness,
    removeHarnessInstall,
    testAcpHarness,
    setChatConfiguration,
    getSkills,
    getSkillContent,
    listLatexTemplates,
    uploadLatexTemplate,
    deleteLatexTemplate,
    listUserSkills,
    uploadUserSkill,
    deleteUserSkill,
    getUserSkillContent,
    updateUserSkill,
    listChatSessions,
    listNativeChats,
    importNativeChat,
    listAllChatSessions,
    searchChatMessages,
    listSidebarChatSessions,
    createChatSession,
    getChatSession,
    deleteChatSession,
    patchChatSession,
    citedMessage,
    openSideChat,
    getChatMessages,
    cancelQueuedMessage,
    retryQueuedMessage,
    sendChatMessage,
    getChatSubmission,
    compactChatSession,
    runShellCommand,
    recoverChatTurn,
    forkChatTurn,
    selectChatBranch,
    interruptChat,
    respondChat,
    getLocalModels,
    discoverLocalModels,
    connectLocalModel,
    checkLocalModel,
    removeLocalModel,
    createAgentProfile,
    updateAgent,
    removeAgentProfile,
    getOpenCodeProviders,
    addOpenCodeProvider,
    replaceOpenCodeProviderKey,
    removeOpenCodeProvider,
    getOpenCodeIntegrations,
    startOpenCodeSignIn,
    getOpenCodeSignIn,
    sendOpenCodeSignInCode,
    cancelOpenCodeSignIn,
    listChatActiveRuns,
    getSubagentTranscript,
  };
}

export type Api = ReturnType<typeof createApi>;
