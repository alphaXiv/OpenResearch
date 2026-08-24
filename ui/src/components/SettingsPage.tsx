import {
  ArrowLeft,
  ChevronDown,
  Cpu,
  ExternalLink,
  Info,
  Monitor,
  Moon,
  Plus,
  RefreshCw,
  Settings,
  SquareTerminal,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  deleteEnvVar,
  fmtBytes,
  fmtDuration,
  getComputeSettings,
  getEnvVars,
  getProjectGitStatus,
  getProjectDefaults,
  getHarnesses,
  getHfSettings,
  getK8sSettings,
  getLocalMachine,
  getModalSettings,
  getOpenResearchSettings,
  getRaySettings,
  getSlurmSettings,
  getSshHosts,
  listInstances,
  setComputeDefault,
  setProjectDefaults,
  provisionModal,
  disableProjectGithub,
  enableProjectGithub,
  initializeProjectGit,
  saveHfToken,
  saveK8sSettings,
  saveRaySettings,
  saveSlurmSettings,
  setEnvVar,
  getDataDir,
  validateDataDir,
  moveDataDir,
  type DataDirSettings,
  type DataDirValidation,
  shortId,
  rayPreflight,
  runDisplayStatus,
  slurmPreflight,
  sshPreflight,
  timeAgo,
  type ComputeSettings,
  type ComputeTargetId,
  type ComputeTargetSummary,
  type EnvVar,
  type Project,
  type ProjectDefaultsSettings,
  type ProjectGitStatus,
  type Harness,
  type HarnessId,
  type HfSettings,
  type HfTokenSource,
  type Instance,
  type K8sSettings,
  type LocalMachine,
  type ModalSettings,
  type ModalTokenSource,
  type OpenResearchSettings,
  type RayPreflight,
  type RaySettings,
  type SlurmPreflight,
  type SlurmSettings,
  type SshHost,
  type SshPreflight,
  applyUpdate,
  harnessModelLabel,
  installCli,
  setAutoUpdate as setAutoUpdateApi,
  type InstallChannel,
  type InstalledCli,
} from "../api";
import { onDataDirMove, onHarnessAuth } from "../events";
import { useUpdateStatus } from "./UpdateBanner";
import { useThemePreference, type ThemePreference } from "../theme";
import { GitTokenForm } from "./GitTokenForm";
import { BackendBadge, BackendLogo } from "./BackendLogos";
import { ProgressBar } from "./ProgressBar";
import { StatusBadge } from "./StatusBadge";
import { BADGE_CLASS_NAME, BUTTON_CLASS_NAME, ERROR_BADGE_CLASS_NAME, ICON_BUTTON_CLASS_NAME, MONO_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SETTINGS_LOADING_CLASS_NAME, SETTINGS_SWITCH_CLASS_NAME, SMALL_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME, SUCCESS_BADGE_CLASS_NAME, WARNING_BADGE_CLASS_NAME } from "../styleClasses";

const SETTINGS_CARD_CLASS_NAME = [
  "settings-card [&_>_.error]:text-accent-red [&_>_.error]:text-md",
  "[&_>_.error]:whitespace-pre-wrap bg-background border border-border",
  "rounded-lg py-4 px-4.5 mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5",
  "[&_h3]:text-sm [&_h3]:font-semibold [&_h3]:text-text",
  "[&_.settings-sub]:mb-3 [&_.kv]:gap-y-1.5 [&_.kv]:gap-x-4.5",
  "[&_>_.project-default-row:first-child]:pt-0 [&_>_.project-default-row:first-child]:border-t-0",
].join(" ");

const KV_CLASS_NAME = [
  "kv grid grid-cols-[auto_1fr] gap-y-[3px] gap-x-3.5 text-md",
  "[&_.k]:text-subtext [&_.v]:font-mono [&_.v]:text-sm",
  "[&_.v]:break-all",
].join(" ");

const SETTINGS_NOTE_CLASS_NAME = [
  "settings-note mt-2.5 mx-0 mb-0 text-sm py-2 px-2.5",
  "border border-accent-amber rounded-md bg-accent-amber-subtle",
  "text-accent-amber font-medium",
].join(" ");

const FORM_CLASS_NAME = [
  "form [&_.form-seg]:self-start [&_.form-seg]:mb-0.5",
  "[&_.form-seg_button]:py-[5px] [&_.form-seg_button]:px-3 [&_.repo-hint]:font-mono",
  "[&_.repo-hint]:font-normal [&_.repo-hint]:text-xs",
  "[&_.repo-hint]:text-muted [&_.repo-hint.ok]:text-accent-teal",
  "[&_.folder-picker-control]:flex [&_.folder-picker-control]:items-center",
  "[&_.folder-picker-control]:gap-[9px] [&_.folder-picker-control]:w-full",
  "[&_.folder-picker-control]:min-w-0 [&_.folder-picker-control]:py-2 [&_.folder-picker-control]:px-2.5",
  "[&_.folder-picker-control]:overflow-hidden [&_.folder-picker-control]:bg-background",
  "[&_.folder-picker-control]:border [&_.folder-picker-control]:border-border",
  "[&_.folder-picker-control]:rounded-md [&_.folder-picker-control]:cursor-pointer",
  "[&_.folder-picker-control]:text-left",
  "[&_.folder-picker-control]:transition-[border-color,box-shadow] [&_.folder-picker-control]:duration-120 [&_.folder-picker-control]:ease-standard",
  "[&_.folder-picker-control:hover:not(:disabled)]:border-muted",
  "[&_.folder-picker-control:hover:not(:disabled)]:shadow-[0_2px_8px_rgb(0_0_0_/_5%)]",
  "[&_.folder-picker-control:focus-visible]:outline-2 [&_.folder-picker-control:focus-visible]:outline-solid [&_.folder-picker-control:focus-visible]:outline-text",
  "[&_.folder-picker-control:focus-visible]:outline-offset-2 [&_.folder-picker-control_span]:flex-1",
  "[&_.folder-picker-control_span]:min-w-0 [&_.folder-picker-control_span]:overflow-hidden",
  "[&_.folder-picker-control_span]:text-ellipsis [&_.folder-picker-control_span]:whitespace-nowrap",
  "[&_.folder-picker-control_.placeholder]:text-muted [&_.folder-picker-icon]:flex-none",
  "[&_.folder-picker-icon]:text-current [&_.folder-picker-chevron]:flex-none",
  "[&_.folder-picker-chevron]:text-muted",
  "[&_.folder-picker-control:hover:not(:disabled)_.folder-picker-chevron]:text-subtext",
  "[&_.folder-picker-hint]:text-subtext [&_.folder-picker-hint]:text-sm",
  "[&_.folder-picker-hint]:font-normal [&_.folder-picker-hint]:leading-[1.4]",
  "[&_.project-location-field]:flex [&_.project-location-field]:flex-col",
  "[&_.project-location-field]:gap-2 [&_.project-location-label]:text-text",
  "[&_.project-location-label]:text-base",
  "[&_.project-location-label]:font-semibold [&_.project-field-label]:text-text",
  "[&_.project-field-label]:text-base [&_.project-field-label]:font-semibold",
  "[&_.folder-picker-control:disabled]:cursor-default [&_.folder-picker-control:disabled]:opacity-65",
  "[&_.paper-destination]:flex [&_.paper-destination]:items-center",
  "[&_.paper-destination]:gap-2.5 [&_.paper-destination]:pt-2 [&_.paper-destination]:pr-2 [&_.paper-destination]:pb-2 [&_.paper-destination]:pl-3",
  "[&_.paper-destination]:border [&_.paper-destination]:border-border [&_.paper-destination]:rounded-md",
  "[&_.paper-destination]:bg-background [&_.paper-destination_code]:flex-1",
  "[&_.paper-destination_code]:min-w-0 [&_.paper-destination_code]:overflow-hidden",
  "[&_.paper-destination_code]:text-text [&_.paper-destination_code]:text-sm",
  "[&_.paper-destination_code]:font-normal",
  "[&_.paper-destination_code]:text-ellipsis [&_.paper-destination_code]:whitespace-nowrap",
  "[&_.paper-destination_.btn]:flex-none [&_.project-path-notice]:py-[9px] [&_.project-path-notice]:px-[11px]",
  "[&_.project-path-notice]:border [&_.project-path-notice]:border-border-variant",
  "[&_.project-path-notice]:rounded-sm [&_.project-path-notice]:bg-surface",
  "[&_.project-path-notice]:text-subtext [&_.project-path-notice]:text-sm",
  "[&_.project-path-notice]:leading-[1.4]",
  "[&_.project-path-notice.error]:border-[color-mix(in_srgb,_var(--accent-red)_35%,_var(--border-variant))]",
  "[&_.paper-results]:flex [&_.paper-results]:flex-col",
  "[&_.paper-results]:border [&_.paper-results]:border-border [&_.paper-results]:rounded-md",
  "[&_.paper-results]:max-h-60 [&_.paper-results]:overflow-y-auto",
  "[&_.paper-results_button]:flex [&_.paper-results_button]:flex-col",
  "[&_.paper-results_button]:items-start [&_.paper-results_button]:gap-0.5",
  "[&_.paper-results_button]:py-2 [&_.paper-results_button]:px-2.5 [&_.paper-results_button]:bg-none [&_.paper-results_button]:bg-transparent",
  "[&_.paper-results_button]:border-0",
  "[&_.paper-results_button]:border-b [&_.paper-results_button]:border-b-border-variant",
  "[&_.paper-results_button]:text-left [&_.paper-results_button]:[font:inherit]",
  "[&_.paper-results_button]:text-text [&_.paper-results_button]:cursor-pointer",
  "[&_.paper-results_button:last-child]:border-b-0",
  "[&_.paper-results_button:hover]:bg-surface [&_.paper-results_.title]:text-md",
  "[&_.paper-results_.title]:font-medium [&_.paper-results_.id]:font-mono",
  "[&_.paper-results_.id]:text-xs [&_.paper-results_.id]:text-muted",
  "[&_.paper-pick_.id]:font-mono [&_.paper-pick_.id]:text-xs",
  "[&_.paper-pick_.id]:text-muted [&_.paper-pick]:flex [&_.paper-pick]:items-center",
  "[&_.paper-pick]:justify-between [&_.paper-pick]:gap-2.5 [&_.paper-pick]:py-2.5 [&_.paper-pick]:px-3",
  "[&_.paper-pick]:border [&_.paper-pick]:border-border [&_.paper-pick]:rounded-md",
  "[&_.paper-pick]:bg-surface [&_.paper-pick_.meta]:min-w-0",
  "[&_.paper-pick_.title]:text-md [&_.paper-pick_.title]:font-semibold",
  "flex flex-col gap-2.5 [&_label]:flex [&_label]:flex-col",
  "[&_label]:gap-1 [&_label]:text-xs [&_label]:text-text",
  "[&_label]:font-medium [&_.row2]:grid [&_.row2]:grid-cols-2",
  "[&_.row2]:gap-2.5 [&_.actions]:flex [&_.actions]:justify-end",
  "[&_.actions]:gap-2.5 [&_.actions]:mt-1.5 [&_.new-project-actions]:justify-start",
  "[&_.new-project-actions]:mt-2.5 [&_.new-project-actions_.primary]:ml-auto",
  "[&_.error]:text-accent-red [&_.error]:text-md [&_.error]:whitespace-pre-wrap",
  "settings-form mt-3.5 pt-3.5 border-t border-t-border",
].join(" ");

const PROJECT_DEFAULT_ROW_CLASS_NAME = [
  "project-default-row flex items-center justify-between gap-6",
  "pt-3.5 border-t border-t-border-variant [&_p]:mt-[3px] [&_p]:mx-0 [&_p]:mb-0",
  "[&_p]:text-muted [&_p]:text-sm",
].join(" ");

const GIT_SETTINGS_CARD_CLASS_NAME = [
  "settings-card [&_>_.error]:text-accent-red [&_>_.error]:text-md",
  "[&_>_.error]:whitespace-pre-wrap bg-background border border-border",
  "rounded-lg mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5 [&_h3]:text-sm",
  "[&_h3]:font-semibold [&_h3]:text-text [&_.settings-sub]:mb-3",
  "[&_>_.project-default-row:first-child]:pt-0 [&_>_.project-default-row:first-child]:border-t-0",
  "git-settings-card py-3.5 px-4 [&_h3]:mb-3",
  "[&_.kv]:grid-cols-[132px_minmax(0,_1fr)] [&_.kv]:items-center [&_.kv]:gap-y-[9px] [&_.kv]:gap-x-4.5",
  "[&_.kv_.k]:text-sm [&_.kv_.v]:flex [&_.kv_.v]:items-center",
  "[&_.kv_.v]:flex-wrap [&_.kv_.v]:gap-[7px] [&_.kv_.v]:min-w-0 [&_.kv_.v]:font-sans",
  "[&_.kv_.v]:text-md [&_.kv_.v]:break-normal [&_.kv_.v.mono]:font-mono",
  "[&_.kv_.v.mono]:text-sm [&_.kv_.v_.mono]:font-mono",
  "[&_.kv_.v_.mono]:text-sm [&_.kv_.k.mono]:font-mono",
  "[&_.kv_.k.mono]:text-sm [@media((max-width:_640px))]:[&_.kv]:grid-cols-1",
  "[@media((max-width:_640px))]:[&_.kv]:gap-[3px] [@media((max-width:_640px))]:[&_.kv_.v_+_.k]:mt-[7px]",
].join(" ");

const GIT_CARD_ACTIONS_CLASS_NAME = [
  "git-card-actions flex flex-wrap gap-2 mt-3.5 pt-3.5",
  "border-t border-t-border-variant",
].join(" ");

const SETTINGS_STACK_SECTION_CLASS_NAME = [
  "settings-stack-section [&_+_.settings-stack-section]:mt-6 [&_>_:last-child]:mb-0",
  "[&_>_h2]:mt-0 [&_>_h2]:mx-0 [&_>_h2]:mb-1.5 [&_>_h2]:text-xl",
].join(" ");

export type SettingsTab =
  | "settings"
  | "harnesses"
  | "projects"
  | "compute"
  | "instances"
  | "environment"
  | "git"
  | "storage";
type Tab = SettingsTab;

// --- harnesses ---------------------------------------------------------------

function harnessStatus(h: Harness): { cls: string; label: string } {
  if (h.agentReady) return { cls: "ok", label: "Signed in" };
  // Not installed — the same blocker whether or not there's saved auth: the
  // CLI has to be installed before anything can run. Amber "action needed".
  if (!h.installed) return { cls: "warn", label: "Not installed" };
  if (h.installBroken) return { cls: "warn", label: "Install broken" };
  if (h.authState === "unknown") return { cls: "warn", label: "Unable to verify" };
  if (h.authState === "unsupported") return { cls: "warn", label: "Update required" };
  return { cls: "warn", label: "Not signed in" };
}

function AuthLabel({ h }: { h: Harness }) {
  if (!h.authMethod) return <>—</>;
  return <>{h.authMethod === "oauth" ? "OAuth (subscription login)" : "API key"}</>;
}

function HarnessesTab() {
  const [harnesses, setHarnesses] = useState<Harness[] | null>(null);
  const [active, setActive] = useState<HarnessId>("claude-code");
  const [refreshing, setRefreshing] = useState(false);

  const load = (refresh: boolean, retryRejected = false) => {
    setRefreshing(true);
    getHarnesses(refresh, retryRejected)
      .then(setHarnesses)
      .catch(() => {})
      .finally(() => setRefreshing(false));
  };
  useEffect(() => load(false), []);
  useEffect(() => onHarnessAuth(() => load(true)), []);

  const h = harnesses?.find((x) => x.id === active);

  return (
    <>
      <h2>Harnesses</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Coding-agent setups detected on this machine. The research agent chat is served by
        OpenCode; Claude Code and Codex accounts surface their models in the composer's model
        picker.
      </p>
      <div className="harness-tabs flex gap-1 mb-3.5 border-b border-b-border-variant [&_button]:inline-flex [&_button]:items-center [&_button]:gap-[7px] [&_button]:py-[7px] [&_button]:px-3 [&_button]:text-md [&_button]:font-semibold [&_button]:text-text [&_button]:border-b-2 [&_button]:border-b-transparent [&_button]:-mb-px [&_button:hover]:text-text [&_button.active]:border-b-primary">
        {(harnesses ?? []).map((x) => (
          <button
            key={x.id}
            className={x.id === active ? "active" : ""}
            onClick={() => setActive(x.id)}
          >
            {x.name}
            <span className={`harness-dot w-[7px] h-[7px] rounded-full bg-muted [&.ok]:bg-accent-green [&.err]:bg-accent-red [&.warn]:bg-accent-amber ${harnessStatus(x).cls}`} />
          </button>
        ))}
      </div>
      {!harnesses ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Detecting harnesses…
        </div>
      ) : !h ? null : (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="settings-card-head flex items-center gap-2.5 mb-3">
            <span className={`${BADGE_CLASS_NAME} ${harnessStatus(h).cls}`}>{harnessStatus(h).label}</span>
            <div className="spacer" style={{ flex: 1 }} />
            <button className={SMALL_BUTTON_CLASS_NAME} onClick={() => load(true, true)} disabled={refreshing}>
              <RefreshCw size={12} className={refreshing ? "spin animate-[settings-spin_0.9s_linear_infinite]" : ""} /> Refresh
            </button>
          </div>
          <div className={KV_CLASS_NAME}>
            <span className="k">Binary</span>
            <span className="v">{h.binPath ?? "not found on PATH"}</span>
            <span className="k">Version</span>
            <span className="v">{h.version ?? "—"}</span>
            <span className="k">Auth</span>
            <span className="v">
              <AuthLabel h={h} />
            </span>
            {h.account && (
              <>
                <span className="k">{h.id === "opencode" ? "Providers" : "Account"}</span>
                <span className="v">{h.account}</span>
              </>
            )}
            {h.org && (
              <>
                <span className="k">Org</span>
                <span className="v">{h.org}</span>
              </>
            )}
            {h.plan && (
              <>
                <span className="k">Plan</span>
                <span className="v">{h.plan}</span>
              </>
            )}
            <span className="k">Agent models</span>
            <span className="v">
              {h.models.length > 0
                ? `${h.models.length} available — ${h.models
                    .slice(0, 4)
                    .map((m) => harnessModelLabel(m))
                    .join(", ")}${h.models.length > 4 ? ", …" : ""}`
                : "none"}
            </span>
          </div>
          {!h.agentReady && h.agentNote && <p className={SETTINGS_NOTE_CLASS_NAME}>{h.agentNote}</p>}
        </div>
      )}
    </>
  );
}

// --- compute (kubernetes) -------------------------------------------------------

function K8sHealthBadge({ s }: { s: K8sSettings }) {
  if (!s.configured) return <span className={BADGE_CLASS_NAME}>Not configured</span>;
  const p = s.preflight;
  if (!p.kubectlFound) return <span className={ERROR_BADGE_CLASS_NAME}>kubectl not found</span>;
  if (!p.reachable) return <span className={ERROR_BADGE_CLASS_NAME}>Cluster unreachable</span>;
  if (!p.canCreateJobs) return <span className={ERROR_BADGE_CLASS_NAME}>No job-create permission</span>;
  return <span className={SUCCESS_BADGE_CLASS_NAME}>Connected</span>;
}

function K8sSection() {
  const [settings, setSettings] = useState<K8sSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [context, setContext] = useState("");
  const [namespace, setNamespace] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const apply = (s: K8sSettings) => {
    setSettings(s);
    setContext(s.context ?? "");
    setNamespace(s.namespace);
  };

  useEffect(() => {
    getK8sSettings()
      .then(apply)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  const unchanged =
    settings !== null &&
    context === (settings.context ?? "") &&
    namespace.trim() === settings.namespace;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      apply(await saveK8sSettings({ context, namespace: namespace.trim() }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run on your own cluster with <code>--backend k8s</code>. The run&apos;s resources
        (image, GPUs, topology) come from a manifest committed on the experiment branch
        (default <code>.orx/k8s.yaml</code>); only the cluster context and namespace live
        here. Auth comes from your kubeconfig.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Checking kubectl…
        </div>
      ) : (
        <>
          <div className={KV_CLASS_NAME}>
            <span className="k">Cluster</span>
            <span className="v">
              <K8sHealthBadge s={settings} />
            </span>
          </div>
          {settings.preflight.error && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>{settings.preflight.error}</p>
          )}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <div className="row2">
              <label>
                Context
                <select value={context} onChange={(e) => setContext(e.target.value)}>
                  <option value="">
                    kubectl default{settings.currentContext ? ` (${settings.currentContext})` : ""}
                  </option>
                  {settings.contexts.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Namespace
                <input
                  className={MONO_CLASS_NAME}
                  type="text"
                  value={namespace}
                  onChange={(e) => setNamespace(e.target.value)}
                  placeholder="default"
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
            </div>
            {error && <div className="error">{error}</div>}
            <div className="actions">
              <button type="submit" className={PRIMARY_BUTTON_CLASS_NAME} disabled={saving || unchanged}>
                {saving ? "Saving…" : "Save"}
              </button>
            </div>
          </form>
          <div className={SETTINGS_CARD_CLASS_NAME}>
            <div className="settings-card-head flex items-center gap-2.5 mb-3">
              <h3>Run manifest</h3>
            </div>
            <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
              Each run applies the manifest committed on its experiment branch — default{" "}
              <code>.orx/k8s.yaml</code>, or <code>--manifest &lt;path&gt;</code>. It declares
              whatever the run needs (image, GPU requests, an Indexed Job across nodes, extra
              Services, …); orx injects the run script as <code>$ORX_SCRIPT</code>, the{" "}
              <code>orx-env</code> Secret, run labels, and a default timeout, and requires
              exactly one Job (or one labelled <code>orx-primary: &quot;true&quot;</code>) whose
              completion is the run&apos;s. Logs follow that Job&apos;s leader pod. Use{" "}
              <code>{"{{ORX_RUN}}"}</code> in resource names to keep re-runs collision-free.
            </p>
          </div>
        </>
      )}
    </>
  );
}

// --- compute (modal) ------------------------------------------------------------

const MODAL_TOKEN_LABELS: Record<ModalTokenSource, string> = {
  env: "MODAL_TOKEN_ID env var",
  syncedEnv: "~/.openresearch/env",
  modalToml: "~/.modal.toml (modal token new)",
};

function ModalBadge({ s }: { s: ModalSettings }) {
  if (s.ready) return <span className={SUCCESS_BADGE_CLASS_NAME}>Connected</span>;
  if (!s.tokenConfigured && !s.modalImportable) return <span className={BADGE_CLASS_NAME}>Not set up</span>;
  if (!s.modalImportable)
    return <span className={ERROR_BADGE_CLASS_NAME}>{s.envProvisioned ? "Env broken" : "Env not built"}</span>;
  if (!s.tokenConfigured) return <span className={ERROR_BADGE_CLASS_NAME}>No token</span>;
  return <span className={BADGE_CLASS_NAME}>Unknown</span>;
}

function ModalSection() {
  const [s, setS] = useState<ModalSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [provisioning, setProvisioning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getModalSettings()
      .then(setS)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  async function provision() {
    if (provisioning) return;
    setProvisioning(true);
    setError(null);
    try {
      setS(await provisionModal());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setProvisioning(false);
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Serverless GPUs on your own Modal account with{" "}
        <code>--backend modal --flavor &lt;name&gt;</code> (t4, a10g, a100-80gb, h100, …). orx
        manages a dedicated Python env with the Modal SDK; sandboxes scale to zero between runs.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !s ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Checking Modal…
        </div>
      ) : (
        <>
          <div className={KV_CLASS_NAME}>
            <span className="k">Status</span>
            <span className="v">
              <ModalBadge s={s} />
            </span>
            <span className="k">Environment</span>
            <span className="v">
              {s.modalImportable
                ? "Ready"
                : s.envProvisioned
                  ? "Provisioned (modal import failing)"
                  : "Not built yet"}
            </span>
            <span className="k">Token</span>
            <span className="v">
              {s.tokenSource ? MODAL_TOKEN_LABELS[s.tokenSource] : "Not configured"}
            </span>
          </div>
          {!s.tokenConfigured && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              No Modal token found. Run <code>modal token new</code>, or add{" "}
              <code>MODAL_TOKEN_ID</code> and <code>MODAL_TOKEN_SECRET</code> in the Environment
              tab.
            </p>
          )}
          {s.error && s.envProvisioned && !s.modalImportable && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>{s.error}</p>
          )}
          {error && <div className="error">{error}</div>}
          {!s.modalImportable && (
            <div className="actions">
              <button className={PRIMARY_BUTTON_CLASS_NAME} onClick={() => void provision()} disabled={provisioning}>
                {provisioning ? "Setting up… (~30–60s)" : "Set up environment"}
              </button>
            </div>
          )}
        </>
      )}
    </>
  );
}

// --- compute (ssh) ---------------------------------------------------------------

type HostTest = "testing" | SshPreflight;

function HostTestCell({ test }: { test: HostTest | undefined }) {
  if (test === undefined) return <span className="muted text-muted">never tested</span>;
  if (test === "testing") return <span className={SPINNER_CLASS_NAME} />;
  const badge = !test.reachable ? (
    <span className={ERROR_BADGE_CLASS_NAME} title={test.error ?? undefined}>Unreachable</span>
  ) : !test.toolsFound ? (
    <span className={ERROR_BADGE_CLASS_NAME}>Missing bash/tar</span>
  ) : (
    <span className={SUCCESS_BADGE_CLASS_NAME}>Ready</span>
  );
  return (
    <>
      {badge}
      <span className="ssh-tested-at block mt-0.5 text-muted text-xs">{timeAgo(test.testedAt)}</span>
    </>
  );
}

function SshSection() {
  const [hosts, setHosts] = useState<SshHost[] | null>(null);
  const [tests, setTests] = useState<Record<string, HostTest>>({});

  useEffect(() => {
    getSshHosts()
      .then(setHosts)
      .catch(() => setHosts([]));
  }, []);

  async function test(host: string) {
    setTests((t) => ({ ...t, [host]: "testing" }));
    try {
      const r = await sshPreflight(host);
      setTests((t) => ({ ...t, [host]: r }));
    } catch (err) {
      setTests((t) => ({
        ...t,
        [host]: {
          reachable: false,
          toolsFound: false,
          error: err instanceof Error ? err.message : String(err),
          testedAt: Date.now(),
        },
      }));
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run experiments directly on your own boxes with{" "}
        <code>--backend ssh --host &lt;alias&gt;</code>. Hosts come from{" "}
        <code>~/.ssh/config</code>; auth uses your keys/agent (orx never reads a key). The host
        just needs <code>git</code> and <code>bash</code>.
      </p>
      {hosts === null ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Reading ~/.ssh/config…
        </div>
      ) : hosts.length === 0 ? (
        <p className="settings-empty text-muted text-md mt-1 mx-0 mb-0">No hosts found in ~/.ssh/config.</p>
      ) : (
        <table className="flavor-table w-full border-collapse text-md [&_th]:pt-[5px] [&_th]:pr-2.5 [&_th]:pb-[5px] [&_th]:pl-0 [&_th]:border-b [&_th]:border-b-border [&_th]:text-left [&_th]:font-medium [&_th]:text-text [&_td]:pt-[5px] [&_td]:pr-2.5 [&_td]:pb-[5px] [&_td]:pl-0 [&_td]:border-b [&_td]:border-b-border-variant ssh-table table-fixed [&_th:nth-child(1)]:w-[20%] [&_th:nth-child(2)]:w-[26%] [&_th:nth-child(4)]:w-27 [&_th:nth-child(5)]:w-13 [&_td]:wrap-anywhere [&_td:last-child]:pr-0 [&_td:last-child]:text-right">
          <thead>
            <tr>
              <th>Host</th>
              <th>Address</th>
              <th>Identity</th>
              <th>Status</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {hosts.map((h) => (
              <tr key={h.host}>
                <td className={MONO_CLASS_NAME}>{h.host}</td>
                <td className={`${MONO_CLASS_NAME} muted text-muted`}>
                  {[h.user, h.hostname ?? "—"].filter(Boolean).join("@")}
                  {h.port ? `:${h.port}` : ""}
                </td>
                <td className={`${MONO_CLASS_NAME} muted text-muted`}>{h.identityFile ?? "—"}</td>
                <td>
                  {/* Session-local result wins; the persisted one covers restarts. */}
                  <HostTestCell test={tests[h.host] ?? h.lastTest} />
                </td>
                <td>
                  <button
                    className={SMALL_BUTTON_CLASS_NAME}
                    onClick={() => void test(h.host)}
                    disabled={tests[h.host] === "testing"}
                  >
                    Test
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

// --- compute (slurm) --------------------------------------------------------------

/** First failing check wins, like K8sHealthBadge. */
function SlurmTestBadge({ test }: { test: "testing" | SlurmPreflight | null }) {
  if (test === null) return null;
  if (test === "testing") return <span className={SPINNER_CLASS_NAME} />;
  if (!test.reachable)
    return (
      <span className={ERROR_BADGE_CLASS_NAME} title={test.error ?? undefined}>
        Unreachable
      </span>
    );
  if (!test.slurmFound) return <span className={ERROR_BADGE_CLASS_NAME}>No Slurm CLI</span>;
  if (!test.toolsFound) return <span className={ERROR_BADGE_CLASS_NAME}>Missing bash/tar</span>;
  return <span className={SUCCESS_BADGE_CLASS_NAME}>Ready</span>;
}

function SlurmSection() {
  const [settings, setSettings] = useState<SlurmSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [host, setHost] = useState("");
  const [partition, setPartition] = useState("");
  const [account, setAccount] = useState("");
  const [timeLimit, setTimeLimit] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<"testing" | SlurmPreflight | null>(null);
  const preflight = test !== null && test !== "testing" ? test : null;

  const apply = (s: SlurmSettings) => {
    setSettings(s);
    setHost(s.host ?? "");
    setPartition(s.partition ?? "");
    setAccount(s.account ?? "");
    setTimeLimit(s.timeLimit ?? "");
  };

  useEffect(() => {
    getSlurmSettings()
      .then(apply)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  const unchanged =
    settings !== null &&
    host === (settings.host ?? "") &&
    partition.trim() === (settings.partition ?? "") &&
    account.trim() === (settings.account ?? "") &&
    timeLimit.trim() === (settings.timeLimit ?? "");

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      apply(
        await saveSlurmSettings({
          host,
          partition: partition.trim(),
          account: account.trim(),
          timeLimit: timeLimit.trim(),
        }),
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function runPreflight(target: string) {
    setTest("testing");
    try {
      setTest(await slurmPreflight(target));
    } catch (err) {
      setTest({
        reachable: false,
        slurmFound: false,
        toolsFound: false,
        partitions: [],
        error: err instanceof Error ? err.message : String(err),
      });
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run on your own cluster with <code>--backend slurm [--flavor h100:2]</code>. orx
        submits via <code>sbatch</code> on the login node over ssh (auth is your keys/agent;
        orx never reads a key) and the job runs in your cluster environment. The defaults
        below apply when a launch doesn&apos;t override them.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading slurm settings…
        </div>
      ) : (
        <>
          {preflight?.error && <p className={SETTINGS_NOTE_CLASS_NAME}>{preflight.error}</p>}
          {preflight && preflight.partitions.length > 0 && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              Partitions: <code>{preflight.partitions.join(", ")}</code>
            </p>
          )}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <div className="row2">
              <label>
                Login node
                <select
                  value={host}
                  onChange={(e) => {
                    setHost(e.target.value);
                    setTest(null); // a badge earned by cluster A must not vouch for cluster B
                  }}
                >
                  <option value="">not set (pass --host per launch)</option>
                  {/* A saved host that has since left ~/.ssh/config still needs an
                      option, or the select renders blank while holding the value. */}
                  {host && !settings.hosts.some((h) => h.host === host) && (
                    <option value={host}>{host} (not in ~/.ssh/config)</option>
                  )}
                  {settings.hosts.map((h) => (
                    <option key={h.host} value={h.host}>
                      {h.host}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Partition
                <input
                  className={MONO_CLASS_NAME}
                  type="text"
                  list="slurm-partitions"
                  value={partition}
                  onChange={(e) => setPartition(e.target.value)}
                  placeholder="cluster default"
                  autoComplete="off"
                  spellCheck={false}
                />
                <datalist id="slurm-partitions">
                  {preflight?.partitions.map((p) => <option key={p} value={p} />)}
                </datalist>
              </label>
            </div>
            <div className="row2">
              <label>
                Account
                <input
                  className={MONO_CLASS_NAME}
                  type="text"
                  value={account}
                  onChange={(e) => setAccount(e.target.value)}
                  placeholder="cluster default"
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
              <label>
                Time limit
                <input
                  className={MONO_CLASS_NAME}
                  type="text"
                  value={timeLimit}
                  onChange={(e) => setTimeLimit(e.target.value)}
                  placeholder="cluster default (e.g. 4h, 30m)"
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
            </div>
            {error && <div className="error">{error}</div>}
            <div className="actions">
              <button type="submit" className={PRIMARY_BUTTON_CLASS_NAME} disabled={saving || unchanged}>
                {saving ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                className={BUTTON_CLASS_NAME}
                onClick={() => void runPreflight(host)}
                disabled={!host || test === "testing"}
                title={host ? undefined : "Pick a login node first"}
              >
                Test connection
              </button>
              <SlurmTestBadge test={test} />
            </div>
          </form>
        </>
      )}
    </>
  );
}

function RaySection() {
  const [settings, setSettings] = useState<RaySettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [address, setAddress] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<"testing" | RayPreflight | null>(null);
  const preflight = test !== null && test !== "testing" ? test : null;

  const apply = (s: RaySettings) => {
    setSettings(s);
    setAddress(s.address ?? "");
  };

  useEffect(() => {
    getRaySettings()
      .then(apply)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  const unchanged = settings !== null && address === (settings.address ?? "");

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      apply(await saveRaySettings({ address }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function runPreflight() {
    setTest("testing");
    try {
      setTest(await rayPreflight(address.trim() || undefined));
    } catch (err) {
      setTest({
        reachable: false,
        address: address.trim() || "(unknown)",
        rayVersion: null,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run on a Ray cluster with <code>--backend ray [--flavor gpu:1]</code>. orx
        submits via the Ray Jobs API (Dashboard URL). Address resolution: this
        setting, then <code>ASTROAI_RAY_JOBS_ADDRESS</code> /{" "}
        <code>RAY_DASHBOARD_URL</code>, then <code>http://127.0.0.1:8265</code>.
        Optional flavor maps to entrypoint CPUs/GPUs/memory (e.g.{" "}
        <code>cpu:2</code>, <code>gpu:1,mem:8GiB</code>).
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading Ray settings…
        </div>
      ) : (
        <>
          <p className={SETTINGS_NOTE_CLASS_NAME}>
            Effective: <code>{settings.resolvedAddress}</code> ({settings.source})
          </p>
          {preflight?.error && <p className={SETTINGS_NOTE_CLASS_NAME}>{preflight.error}</p>}
          {preflight?.reachable && preflight.rayVersion && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              Ray version: <code>{preflight.rayVersion}</code>
            </p>
          )}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <label>
              Jobs / Dashboard URL
              <input
                className={MONO_CLASS_NAME}
                type="text"
                value={address}
                onChange={(e) => {
                  setAddress(e.target.value);
                  setTest(null);
                }}
                placeholder="http://127.0.0.1:8265"
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            {error && <div className="error">{error}</div>}
            <div className="actions">
              <button type="submit" className={PRIMARY_BUTTON_CLASS_NAME} disabled={saving || unchanged}>
                {saving ? "Saving…" : "Save"}
              </button>
              <button
                type="button"
                className={BUTTON_CLASS_NAME}
                onClick={() => void runPreflight()}
                disabled={test === "testing"}
              >
                Test connection
              </button>
              <RayTestBadge test={test} />
            </div>
          </form>
        </>
      )}
    </>
  );
}

function RayTestBadge({ test }: { test: "testing" | RayPreflight | null }) {
  if (test === null) return null;
  if (test === "testing") return <span className={BADGE_CLASS_NAME}>Testing…</span>;
  if (test.reachable) return <span className={SUCCESS_BADGE_CLASS_NAME}>Reachable</span>;
  return <span className={WARNING_BADGE_CLASS_NAME}>Unreachable</span>;
}

// --- compute (local) --------------------------------------------------------------

function LocalSection() {
  const [hw, setHw] = useState<LocalMachine | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    getLocalMachine()
      .then(setHw)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run experiments as detached, supervised processes on the machine running orx with{" "}
        <code>--backend local</code> — handy when you&apos;re already on a GPU box and using
        this dashboard over port forwarding. Runs share CPU/RAM/GPU with the dashboard
        itself, so prefer a remote backend for anything heavy.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !hw ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Detecting hardware…
        </div>
      ) : (
        <div className={KV_CLASS_NAME}>
          <span className="k">Hostname</span>
          <span className={`v ${MONO_CLASS_NAME}`}>{hw.hostname}</span>
          <span className="k">System</span>
          <span className="v">
            {hw.os}/{hw.arch}
            {hw.chip ? ` — ${hw.chip}` : ""}
          </span>
          <span className="k">CPU</span>
          <span className="v">{hw.cpuCount > 0 ? `${hw.cpuCount} cores` : "—"}</span>
          <span className="k">RAM</span>
          <span className="v">{hw.memBytes !== null ? fmtBytes(hw.memBytes) : "—"}</span>
          <span className="k">GPUs</span>
          <span className="v">
            {hw.gpus.length === 0
              ? "none detected (nvidia-smi)"
              : hw.gpus
                  .map(
                    (g) =>
                      `${g.name}${g.memMib !== null ? ` — ${fmtBytes(g.memMib * 1024 * 1024)}` : ""}`,
                  )
                  .join(", ")}
          </span>
        </div>
      )}
    </>
  );
}

// --- compute (openresearch) ---------------------------------------------------------

function OpenResearchSection() {
  const [s, setS] = useState<OpenResearchSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    getOpenResearchSettings()
      .then(setS)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run on an ephemeral OpenResearch box billed to your org with{" "}
        <code>--backend openresearch --flavor &lt;shape&gt;</code> (h100_sxm, cpu5c, …; browse
        with <code>orx compute</code>). The box is provisioned for the run and deleted when it
        ends. Needs <code>orx login</code> and a registered SSH key.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !s ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Checking credentials…
        </div>
      ) : !s.loggedIn ? (
        <p className={SETTINGS_NOTE_CLASS_NAME}>
          Not signed in. Run <code>orx login</code> in a terminal to connect your OpenResearch
          account.
        </p>
      ) : (
        <>
          <div className={KV_CLASS_NAME}>
            <span className="k">Status</span>
            <span className="v">
              <span className={SUCCESS_BADGE_CLASS_NAME}>Signed in</span>
            </span>
            <span className="k">Orgs</span>
            <span className="v">{s.orgs.length > 0 ? s.orgs.join(", ") : "—"}</span>
            <span className="k">SSH key</span>
            <span className="v">
              {s.sshKeyStatus === "matched" ? (
                <span className={SUCCESS_BADGE_CLASS_NAME}>On this computer</span>
              ) : s.sshKeyStatus === "no_local_match" ? (
                <span className={WARNING_BADGE_CLASS_NAME}>Not on this computer</span>
              ) : s.sshKeyStatus === "none_registered" ? (
                <span className={ERROR_BADGE_CLASS_NAME}>None registered</span>
              ) : (
                <span className={BADGE_CLASS_NAME}>Unknown</span>
              )}
            </span>
          </div>
          {s.sshKeyStatus === "none_registered" &&
            (s.sshKeyPath ? (
              <p className={SETTINGS_NOTE_CLASS_NAME}>
                Add one with <code>orx ssh-key add {s.sshKeyPath}</code>.
              </p>
            ) : (
              <p className={SETTINGS_NOTE_CLASS_NAME}>
                No key on this computer yet — create one with{" "}
                <code>ssh-keygen -t ed25519</code>, then add it with{" "}
                <code>orx ssh-key add</code>.
              </p>
            ))}
          {s.sshKeyStatus === "no_local_match" &&
            (s.sshKeyPath ? (
              <p className={SETTINGS_NOTE_CLASS_NAME}>
                Register this computer with <code>orx ssh-key add {s.sshKeyPath}</code>,
                or load a registered key with <code>ssh-add</code>.
              </p>
            ) : (
              <p className={SETTINGS_NOTE_CLASS_NAME}>
                No key on this computer to register — load a registered key with{" "}
                <code>ssh-add</code>, or create one with{" "}
                <code>ssh-keygen -t ed25519</code>.
              </p>
            ))}
          {s.error && <p className={SETTINGS_NOTE_CLASS_NAME}>{s.error}</p>}
        </>
      )}
    </>
  );
}

// --- compute -----------------------------------------------------------------

const TARGET_LABELS: Record<ComputeTargetId, string> = {
  local: "This machine",
  hf: "HF Jobs",
  modal: "Modal",
  k8s: "Kubernetes",
  ssh: "SSH",
  slurm: "Slurm",
  ray: "Ray",
  openresearch: "OpenResearch",
};

/** Kind strings from the runs table — reuses the instances-table logos. */
const TARGET_KIND: Record<ComputeTargetId, string> = {
  local: "local_job",
  hf: "hf_job",
  modal: "modal_job",
  k8s: "k8s_job",
  ssh: "ssh_job",
  slurm: "slurm_job",
  ray: "ray_job",
  openresearch: "openresearch_job",
};

/** Backends whose launches take --flavor; mirrors the server's validation. */
const FLAVORED_TARGETS: ComputeTargetId[] = ["hf", "modal", "slurm", "ray", "openresearch"];
/** Of those, the ones where a launch *requires* a flavor. */
const FLAVOR_REQUIRED: ComputeTargetId[] = ["hf", "modal", "openresearch"];

const FLAVOR_SUGGESTIONS: Partial<Record<ComputeTargetId, string[]>> = {
  hf: ["cpu-basic", "t4-small", "a10g-small", "a10g-large", "a100-large", "h100", "h200"],
  modal: ["cpu", "t4", "l4", "a10g", "a100", "a100-80gb", "l40s", "h100", "h100:2"],
  slurm: ["gpu", "h100:1", "h100:2", "a100:4"],
  ray: ["cpu", "cpu:2", "gpu", "gpu:1", "gpu:1,cpu:4", "gpu:1,mem:8GiB"],
  openresearch: ["h100_sxm", "h100_sxm:2", "cpu5c", "cpu5g", "cpu5m"],
};

function TargetStatusBadge({ t, isDefault }: { t: ComputeTargetSummary; isDefault: boolean }) {
  if (t.id === "local") return <span className={SUCCESS_BADGE_CLASS_NAME}>Ready</span>;
  // Don't claim either answer when the check couldn't run.
  if (t.unverified) return <span className={BADGE_CLASS_NAME}>Unknown</span>;
  if (!t.configured && isDefault) return <span className={WARNING_BADGE_CLASS_NAME}>Not configured</span>;
  if (!t.configured) return <span className={BADGE_CLASS_NAME}>Not set up</span>;
  return <span className={SUCCESS_BADGE_CLASS_NAME}>Configured</span>;
}

/** The default row's inline flavor editor (flavored backends only). */
function DefaultFlavorEditor({
  target,
  flavor,
  projectId,
  onSaved,
}: {
  target: ComputeTargetId;
  flavor: string | null;
  projectId?: string;
  onSaved: (s: ComputeSettings) => void;
}) {
  const [value, setValue] = useState(flavor ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Reflect an outside change (e.g. default moved to another backend and back).
  useEffect(() => setValue(flavor ?? ""), [flavor]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      onSaved(await setComputeDefault({ backend: target, flavor: value.trim() || null, projectId }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  const unchanged = value.trim() === (flavor ?? "");
  return (
    <form className="form [&_.form-seg]:self-start [&_.form-seg]:mb-0.5 [&_.form-seg_button]:py-[5px] [&_.form-seg_button]:px-3 [&_.repo-hint]:font-mono [&_.repo-hint]:font-normal [&_.repo-hint]:text-xs [&_.repo-hint]:text-muted [&_.repo-hint.ok]:text-accent-teal [&_.folder-picker-control]:flex [&_.folder-picker-control]:items-center [&_.folder-picker-control]:gap-[9px] [&_.folder-picker-control]:w-full [&_.folder-picker-control]:min-w-0 [&_.folder-picker-control]:py-2 [&_.folder-picker-control]:px-2.5 [&_.folder-picker-control]:overflow-hidden [&_.folder-picker-control]:bg-background [&_.folder-picker-control]:border [&_.folder-picker-control]:border-border [&_.folder-picker-control]:rounded-md [&_.folder-picker-control]:cursor-pointer [&_.folder-picker-control]:text-left [&_.folder-picker-control]:transition-[border-color,box-shadow] [&_.folder-picker-control]:duration-120 [&_.folder-picker-control]:ease-standard [&_.folder-picker-control:hover:not(:disabled)]:border-muted [&_.folder-picker-control:hover:not(:disabled)]:shadow-[0_2px_8px_rgb(0_0_0_/_5%)] [&_.folder-picker-control:focus-visible]:outline-2 [&_.folder-picker-control:focus-visible]:outline-solid [&_.folder-picker-control:focus-visible]:outline-text [&_.folder-picker-control:focus-visible]:outline-offset-2 [&_.folder-picker-control_span]:flex-1 [&_.folder-picker-control_span]:min-w-0 [&_.folder-picker-control_span]:overflow-hidden [&_.folder-picker-control_span]:text-ellipsis [&_.folder-picker-control_span]:whitespace-nowrap [&_.folder-picker-control_.placeholder]:text-muted [&_.folder-picker-icon]:flex-none [&_.folder-picker-icon]:text-current [&_.folder-picker-chevron]:flex-none [&_.folder-picker-chevron]:text-muted [&_.folder-picker-control:hover:not(:disabled)_.folder-picker-chevron]:text-subtext [&_.folder-picker-hint]:text-subtext [&_.folder-picker-hint]:text-sm [&_.folder-picker-hint]:font-normal [&_.folder-picker-hint]:leading-[1.4] [&_.project-location-field]:flex [&_.project-location-field]:flex-col [&_.project-location-field]:gap-2 [&_.project-location-label]:text-text [&_.project-location-label]:text-base [&_.project-location-label]:font-semibold [&_.project-field-label]:text-text [&_.project-field-label]:text-base [&_.project-field-label]:font-semibold [&_.folder-picker-control:disabled]:cursor-default [&_.folder-picker-control:disabled]:opacity-65 [&_.paper-destination]:flex [&_.paper-destination]:items-center [&_.paper-destination]:gap-2.5 [&_.paper-destination]:pt-2 [&_.paper-destination]:pr-2 [&_.paper-destination]:pb-2 [&_.paper-destination]:pl-3 [&_.paper-destination]:border [&_.paper-destination]:border-border [&_.paper-destination]:rounded-md [&_.paper-destination]:bg-background [&_.paper-destination_code]:flex-1 [&_.paper-destination_code]:min-w-0 [&_.paper-destination_code]:overflow-hidden [&_.paper-destination_code]:text-text [&_.paper-destination_code]:text-sm [&_.paper-destination_code]:font-normal [&_.paper-destination_code]:text-ellipsis [&_.paper-destination_code]:whitespace-nowrap [&_.paper-destination_.btn]:flex-none [&_.project-path-notice]:py-[9px] [&_.project-path-notice]:px-[11px] [&_.project-path-notice]:border [&_.project-path-notice]:border-border-variant [&_.project-path-notice]:rounded-sm [&_.project-path-notice]:bg-surface [&_.project-path-notice]:text-subtext [&_.project-path-notice]:text-sm [&_.project-path-notice]:leading-[1.4] [&_.project-path-notice.error]:border-[color-mix(in_srgb,_var(--accent-red)_35%,_var(--border-variant))] [&_.paper-results]:flex [&_.paper-results]:flex-col [&_.paper-results]:border [&_.paper-results]:border-border [&_.paper-results]:rounded-md [&_.paper-results]:max-h-60 [&_.paper-results]:overflow-y-auto [&_.paper-results_button]:flex [&_.paper-results_button]:flex-col [&_.paper-results_button]:items-start [&_.paper-results_button]:gap-0.5 [&_.paper-results_button]:py-2 [&_.paper-results_button]:px-2.5 [&_.paper-results_button]:bg-none [&_.paper-results_button]:bg-transparent [&_.paper-results_button]:border-0 [&_.paper-results_button]:border-b [&_.paper-results_button]:border-b-border-variant [&_.paper-results_button]:text-left [&_.paper-results_button]:[font:inherit] [&_.paper-results_button]:text-text [&_.paper-results_button]:cursor-pointer [&_.paper-results_button:last-child]:border-b-0 [&_.paper-results_button:hover]:bg-surface [&_.paper-results_.title]:text-md [&_.paper-results_.title]:font-medium [&_.paper-results_.id]:font-mono [&_.paper-results_.id]:text-xs [&_.paper-results_.id]:text-muted [&_.paper-pick_.id]:font-mono [&_.paper-pick_.id]:text-xs [&_.paper-pick_.id]:text-muted [&_.paper-pick]:flex [&_.paper-pick]:items-center [&_.paper-pick]:justify-between [&_.paper-pick]:gap-2.5 [&_.paper-pick]:py-2.5 [&_.paper-pick]:px-3 [&_.paper-pick]:border [&_.paper-pick]:border-border [&_.paper-pick]:rounded-md [&_.paper-pick]:bg-surface [&_.paper-pick_.meta]:min-w-0 [&_.paper-pick_.title]:text-md [&_.paper-pick_.title]:font-semibold flex flex-col gap-2.5 [&_label]:flex [&_label]:flex-col [&_label]:gap-1 [&_label]:text-xs [&_label]:text-text [&_label]:font-medium [&_.row2]:grid [&_.row2]:grid-cols-2 [&_.row2]:gap-2.5 [&_.actions]:flex [&_.actions]:justify-end [&_.actions]:gap-2.5 [&_.actions]:mt-1.5 [&_.new-project-actions]:justify-start [&_.new-project-actions]:mt-2.5 [&_.new-project-actions_.primary]:ml-auto [&_.error]:text-accent-red [&_.error]:text-md [&_.error]:whitespace-pre-wrap settings-form mt-3.5 pt-3.5 border-t border-t-border compute-flavor-form mb-3.5 [&_label]:max-w-80" onSubmit={submit}>
      <label>
        Default flavor
        <input
          className={MONO_CLASS_NAME}
          type="text"
          list={`flavors-${target}`}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={
            FLAVOR_REQUIRED.includes(target)
              ? `e.g. ${FLAVOR_SUGGESTIONS[target]?.[1] ?? ""}`
              : "none (CPU-only)"
          }
          autoComplete="off"
          spellCheck={false}
        />
        <datalist id={`flavors-${target}`}>
          {(FLAVOR_SUGGESTIONS[target] ?? []).map((f) => (
            <option key={f} value={f} />
          ))}
        </datalist>
      </label>
      {error && <div className="error">{error}</div>}
      <div className="actions">
        <button type="submit" className={SMALL_BUTTON_CLASS_NAME} disabled={saving || unchanged}>
          {saving ? "Saving…" : "Save flavor"}
        </button>
        {FLAVOR_REQUIRED.includes(target) && !flavor && (
          <span className="muted text-muted compute-flavor-hint text-sm">
            This backend requires a flavor — without a default one, each launch must pass{" "}
            <code>--flavor</code>.
          </span>
        )}
      </div>
    </form>
  );
}

function TargetRow({
  target,
  isDefault,
  isFallbackDefault,
  defaultFlavor,
  open,
  onToggle,
  onSettings,
  onError,
  projectId,
}: {
  target: ComputeTargetSummary;
  isDefault: boolean;
  isFallbackDefault: boolean;
  defaultFlavor: string | null;
  open: boolean;
  onToggle: () => void;
  onSettings: (s: ComputeSettings) => void;
  onError: (msg: string) => void;
  projectId?: string;
}) {
  // Mounted on first expand, kept mounted (hidden) after — each section's own
  // mount-time fetch is the lazy detail load, and re-expanding doesn't refetch.
  const [visited, setVisited] = useState(false);
  const [settingDefault, setSettingDefault] = useState(false);
  if (open && !visited) setVisited(true);

  async function setDefault(backend: ComputeTargetId | null) {
    if (settingDefault) return;
    setSettingDefault(true);
    try {
      onSettings(await setComputeDefault({ backend, projectId }));
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setSettingDefault(false);
    }
  }

  return (
    <div className={`compute-row bg-background border border-border rounded-lg [&.disabled]:opacity-52 [&.disabled_.compute-row-head]:cursor-default [&.open_.compute-row-head:hover]:rounded-[var(--radius-lg)_var(--radius-lg)_0_0] [&.open_.compute-chevron]:rotate-180${open ? " open" : ""}${target.enabled ? "" : " disabled"}`}>
      {/* The head is a plain clickable div, NOT role="button": it holds real
          buttons (Make default, the chevron), and interactive elements must
          not nest. The chevron is the keyboard-reachable expand control. */}
      <div className="compute-row-head flex items-center gap-2.5 py-3 px-3.5 cursor-pointer select-none [&:hover]:bg-surface [&:hover]:rounded-lg [&_.badge]:flex-none" onClick={target.enabled ? onToggle : undefined}>
        <span className="compute-row-logo inline-flex items-center flex-none">
          <BackendLogo kind={TARGET_KIND[target.id]} size={18} />
        </span>
        <span className="compute-row-name text-md font-semibold text-text flex-none">{TARGET_LABELS[target.id]}</span>
        <span className="compute-row-summary flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-muted text-sm">{target.summary}</span>
        <TargetStatusBadge t={target} isDefault={isDefault} />
        {isDefault ? (
          <span className="badge inline-flex items-center font-sans text-xs font-medium py-px px-[7px] border border-border rounded-sm [&.ok]:text-accent-green [&.ok]:border-accent-green [&.ok]:bg-accent-green-subtle [&.err]:text-accent-red [&.err]:border-accent-red [&.err]:bg-accent-red-subtle [&.warn]:text-accent-amber [&.warn]:border-accent-amber [&.warn]:bg-accent-amber-subtle compute-default-pill flex-none text-primary border-primary">{isFallbackDefault ? "Local fallback" : "Default"}</span>
        ) : (
          <button
            type="button"
            className={`${SMALL_BUTTON_CLASS_NAME} compute-make-default flex-none`}
            onClick={(e) => {
              e.stopPropagation(); // the header click is expand/collapse
              void setDefault(target.id);
            }}
            disabled={settingDefault || !target.enabled}
          >
            Make default
          </button>
        )}
        <button
          type="button"
          className="compute-chevron-btn flex-none inline-flex items-center p-0.5 rounded-sm [&:hover]:bg-panel"
          aria-expanded={open}
          aria-label={`${open ? "Collapse" : "Expand"} ${TARGET_LABELS[target.id]}`}
          disabled={!target.enabled}
          onClick={(e) => {
            e.stopPropagation();
            if (target.enabled) onToggle();
          }}
        >
          <ChevronDown size={16} className="compute-chevron text-muted transition-transform duration-120 ease-standard" />
        </button>
      </div>
      {visited && target.enabled && (
        <div className="compute-row-body border-t border-t-border p-3.5 [&_.settings-card]:mb-0 [&_.settings-card]:mt-3.5" hidden={!open}>
          {isDefault && !isFallbackDefault && (
            <p className="settings-note mt-2.5 mx-0 mb-0 text-sm py-2 px-2.5 border border-accent-amber rounded-md bg-accent-amber-subtle text-accent-amber font-medium compute-default-note flex items-center gap-2.5 flex-wrap">
              The agent launches runs here unless you tell it otherwise, and so does{" "}
              <code>orx exp run</code> with no <code>--backend</code> flag.{" "}
              <button
                type="button"
                className={SMALL_BUTTON_CLASS_NAME}
                onClick={() => void setDefault(null)}
                disabled={settingDefault}
              >
                Clear default
              </button>
            </p>
          )}
          {isDefault && !target.configured && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              This target is the default but isn&apos;t configured — launches will fail until
              it&apos;s set up below.
            </p>
          )}
          {isDefault && FLAVORED_TARGETS.includes(target.id) && (
            <DefaultFlavorEditor target={target.id} flavor={defaultFlavor} projectId={projectId} onSaved={onSettings} />
          )}
          {target.id === "local" && <LocalSection />}
          {target.id === "hf" && <HfSection />}
          {target.id === "modal" && <ModalSection />}
          {target.id === "k8s" && <K8sSection />}
          {target.id === "ssh" && <SshSection />}
          {target.id === "slurm" && <SlurmSection />}
          {target.id === "ray" && <RaySection />}
          {target.id === "openresearch" && <OpenResearchSection />}
        </div>
      )}
    </div>
  );
}

function ComputeTab({
  project,
  onViewHistory,
}: {
  project: Project | null;
  onViewHistory: () => void;
}) {
  const [settings, setSettings] = useState<ComputeSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<ComputeTargetId | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Monotonic guard: a POST response applied via `apply` must not be
  // overwritten by a slower background GET that was already in flight.
  const seqRef = useRef(0);

  useEffect(() => {
    seqRef.current++;
    setSettings(null);
    setExpanded(null);
    setLoadError(null);
    setError(null);
  }, [project?.id]);

  // Refetched whenever a row expands/collapses (not just on mount): a form
  // saved inside a row (k8s context, HF token, …) changes the collapsed
  // summaries, and the toggle is the natural moment to catch up. Cheap by
  // contract — the endpoint only does fs/env probes.
  useEffect(() => {
    const seq = ++seqRef.current;
    getComputeSettings(project?.id)
      .then((s) => {
        if (seq !== seqRef.current) return;
        setSettings(s);
        setLoadError(null);
      })
      .catch((err) => {
        if (seq !== seqRef.current) return;
        // Only the very first load may brick the tab; a failed background
        // refresh of already-rendered rows goes to the transient banner.
        const msg = err instanceof Error ? err.message : String(err);
        setSettings((cur) => {
          if (cur === null) setLoadError(msg);
          else setError(msg);
          return cur;
        });
      });
  }, [expanded, project?.id]);

  const apply = (s: ComputeSettings) => {
    seqRef.current++; // supersede any in-flight background GET
    setSettings(s);
    setError(null);
  };

  // Server order is canonical (local first, then external backends).
  const targets = settings ? settings.targets : null;
  const fallbackDefault =
    settings?.defaultBackend === "local" &&
    settings.configuredDefaultBackend !== null &&
    settings.configuredDefaultBackend !== undefined &&
    settings.configuredDefaultBackend !== "local";
  const renderTarget = (target: ComputeTargetSummary) => (
    <TargetRow
      key={`${project?.id ?? "none"}:${target.id}`}
      target={target}
      isDefault={settings?.defaultBackend === target.id}
      isFallbackDefault={Boolean(fallbackDefault && target.id === "local")}
      defaultFlavor={settings?.defaultFlavor ?? null}
      open={target.enabled && expanded === target.id}
      onToggle={() => setExpanded((current) => (current === target.id ? null : target.id))}
      onSettings={apply}
      onError={setError}
      projectId={project?.id}
    />
  );

  return (
    <>
      <h1>Compute</h1>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Where <code>orx exp run</code> executes. Pick a default target; the agent uses it when
        a launch doesn&apos;t name a backend (<code>--backend &lt;name&gt;</code> always wins).
      </p>
      <ComputeActivity onViewHistory={onViewHistory} />
      <h2 className="compute-section-title mt-0 mx-0 mb-2.5 text-lg">Targets</h2>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !targets ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Checking compute targets…
        </div>
      ) : (
        <>
          {error && <div className="error">{error}</div>}
          <div className="compute-list flex flex-col gap-2.5 mb-3.5">
            {targets.filter((target) => target.id === "local").map(renderTarget)}
            {targets.filter((target) => target.id !== "local").map(renderTarget)}
          </div>
          {fallbackDefault && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              Using this machine because the saved {settings?.configuredDefaultBackend} default is not currently configured.
            </p>
          )}
          <p className="compute-footnote flex items-start gap-1.5 mt-0.5 mx-0 mb-0 text-sm text-muted [&_svg]:flex-none [&_svg]:mt-px">
            <Info size={14} aria-hidden="true" />
            <span>
              The default target and flavor are included in the research agent&apos;s
              instructions — it launches runs there unless you name another backend. No other
              compute settings are shared with it.
            </span>
          </p>
        </>
      )}
    </>
  );
}

// --- environment ---------------------------------------------------------------

const SOURCE_LABELS: Record<HfTokenSource, string> = {
  env: "HF_TOKEN env var",
  openresearchEnv: "~/.openresearch/env",
  hfCache: "~/.cache/huggingface/token (hf auth login)",
};

function HfStatusBadge({ settings }: { settings: HfSettings }) {
  if (!settings.configured) return <span className={BADGE_CLASS_NAME}>Not configured</span>;
  if (!settings.valid) return <span className={ERROR_BADGE_CLASS_NAME}>Invalid token</span>;
  return <span className={SUCCESS_BADGE_CLASS_NAME}>Connected</span>;
}

/** Jobs-permission detail only — configured/valid state is HfStatusBadge's job. */
function HfJobsBadge({ settings }: { settings: HfSettings }) {
  if (!settings.configured || !settings.valid) return null;
  if (settings.jobsWrite === true) return <span className={SUCCESS_BADGE_CLASS_NAME}>Jobs: write OK</span>;
  if (settings.jobsWrite === false)
    return <span className={ERROR_BADGE_CLASS_NAME}>No job.write permission</span>;
  return <span className={BADGE_CLASS_NAME}>Jobs permission unknown</span>;
}

function HfSection() {
  const [settings, setSettings] = useState<HfSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [token, setToken] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A save that lands before the slow mount fetch resolves must win over it.
  const savedRef = useRef(false);

  // Fetched on mount (every visit remounts) so a token set anywhere else —
  // the Environment tab, `hf auth login`, the process env — shows up here.
  useEffect(() => {
    getHfSettings()
      .then((s) => {
        if (!savedRef.current) setSettings(s);
      })
      .catch((err) => {
        if (!savedRef.current) setLoadError(err instanceof Error ? err.message : String(err));
      });
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!token.trim() || saving) return;
    setSaving(true);
    setError(null);
    try {
      const next = await saveHfToken(token.trim());
      savedRef.current = true;
      setSettings(next);
      setLoadError(null);
      setToken("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Run experiments on your Hugging Face account with{" "}
        <code>--backend hf --flavor &lt;name&gt;</code> (t4-small, a10g-small, a100-large, …).
        Billed to HF per minute.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading status…
        </div>
      ) : (
        <>
          <div className={KV_CLASS_NAME}>
            <span className="k">Status</span>
            <span className="v">
              <HfStatusBadge settings={settings} />
            </span>
            <span className="k">Account</span>
            <span className="v">{settings.username ?? "—"}</span>
            <span className="k">Token</span>
            <span className="v">{settings.maskedToken ?? "—"}</span>
            <span className="k">Source</span>
            <span className="v">
              {settings.source ? SOURCE_LABELS[settings.source] : "Not configured"}
            </span>
            <span className="k">Jobs</span>
            <span className="v">
              <HfJobsBadge settings={settings} />
              {(!settings.configured || !settings.valid) && "—"}
            </span>
          </div>
          {settings.source === "env" && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              HF_TOKEN is set in the environment and overrides any token saved here.
            </p>
          )}
          {settings.valid && settings.jobsWrite === null && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              This token is valid but doesn&apos;t report whether it can launch Jobs — OAuth
              tokens from <code>hf auth login</code> never do. Launches may still work; for a
              definitive check, save a write-scoped token from{" "}
              <a href="https://huggingface.co/settings/tokens" target="_blank" rel="noreferrer">
                huggingface.co/settings/tokens
              </a>
              .
            </p>
          )}
        </>
      )}
      <form className={FORM_CLASS_NAME} onSubmit={submit}>
        <label>
          {settings?.configured ? "Replace token" : "New token"}
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="hf_…"
            autoComplete="off"
          />
        </label>
        {error && <div className="error">{error}</div>}
        <div className="actions">
          <button type="submit" className={PRIMARY_BUTTON_CLASS_NAME} disabled={!token.trim() || saving}>
            {saving ? "Validating…" : "Save"}
          </button>
        </div>
      </form>
    </>
  );
}

// HF user access tokens are `hf_` + alphanumeric. Compute runs resolve the
// token strictly by the name HF_TOKEN, so an hf_… value saved under any other
// key is invisible to them — worth a (non-blocking) warning.
const HF_TOKEN_RE = /^hf_[A-Za-z0-9]{10,}$/;

/** The wrong-key warning shown when an hf_… value is headed somewhere else. */
function HfHintRow() {
  return (
    <tr>
      {/* colSpan tracks the EnvRow/AddVarRow column count */}
      <td colSpan={3}>
        <p className={SETTINGS_NOTE_CLASS_NAME}>
          This value looks like a Hugging Face token — compute runs only read it from{" "}
          <code>HF_TOKEN</code>. Save it under that key if it&apos;s meant for HF Jobs.
        </p>
      </td>
    </tr>
  );
}

// Keys runs typically need (HF_TOKEN is also read by orx itself), always
// shown as rows alongside custom variables.
const RECOMMENDED_ENV_KEYS = ["HF_TOKEN", "WANDB_API_KEY"];

/** One variable row. Set: masked value + delete. Unset: inline value input. */
function EnvRow({
  name,
  entry,
  onVars,
  onError,
}: {
  name: string;
  entry: EnvVar | undefined;
  onVars: (vars: EnvVar[]) => void;
  onError: (msg: string) => void;
}) {
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);

  // Errors share one card-level slot, so name the row they came from.
  const fail = (err: unknown) =>
    onError(`${name}: ${err instanceof Error ? err.message : String(err)}`);

  async function save() {
    if (!value.trim() || saving) return;
    setSaving(true);
    try {
      onVars(await setEnvVar(name, value.trim()));
      setValue("");
    } catch (err) {
      fail(err);
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (saving) return;
    setSaving(true);
    try {
      onVars(await deleteEnvVar(name));
    } catch (err) {
      fail(err);
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <tr>
        <td className={MONO_CLASS_NAME}>{name}</td>
        <td className={`${MONO_CLASS_NAME} muted text-muted`}>
          {entry ? (
            <>
              {entry.maskedValue}
              {entry.inProcessEnv && <span className={BADGE_CLASS_NAME}>Overridden by env</span>}
            </>
          ) : (
            <input
              className={MONO_CLASS_NAME}
              type="password"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void save();
                }
                if (e.key === "Escape" && !saving) setValue("");
              }}
              placeholder="value"
              aria-label={`Value for ${name}`}
              autoComplete="new-password"
              disabled={saving}
            />
          )}
        </td>
        <td>
          {entry ? (
            <button
              className={ICON_BUTTON_CLASS_NAME}
              title={`Delete ${name}`}
              aria-label={`Delete ${name}`}
              onClick={() => void remove()}
              disabled={saving}
            >
              <Trash2 size={13} />
            </button>
          ) : (
            value.trim() && (
              <button className={SMALL_BUTTON_CLASS_NAME} onClick={() => void save()} disabled={saving}>
                {saving ? "Saving…" : "Save"}
              </button>
            )
          )}
        </td>
      </tr>
      {!entry && name !== "HF_TOKEN" && HF_TOKEN_RE.test(value.trim()) && <HfHintRow />}
    </>
  );
}

/** The in-table row for a new custom variable (opened by “Add variable”). */
function AddVarRow({
  onVars,
  onError,
  onDone,
}: {
  onVars: (vars: EnvVar[]) => void;
  onError: (msg: string) => void;
  onDone: () => void;
}) {
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!key.trim() || !value.trim() || saving) return;
    setSaving(true);
    try {
      onVars(await setEnvVar(key.trim(), value.trim()));
      onDone();
    } catch (err) {
      onError(`${key.trim()}: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setSaving(false);
    }
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void save();
    }
    if (e.key === "Escape" && !saving) onDone();
  };

  return (
    <>
      <tr>
        <td>
          <input
            autoFocus
            className={MONO_CLASS_NAME}
            type="text"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="MY_API_KEY"
            aria-label="New variable key"
            autoComplete="off"
            spellCheck={false}
            disabled={saving}
          />
        </td>
        <td>
          <input
            className={MONO_CLASS_NAME}
            type="password"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="value"
            aria-label="New variable value"
            autoComplete="new-password"
            disabled={saving}
          />
        </td>
        <td>
          <button
            className={SMALL_BUTTON_CLASS_NAME}
            onClick={() => void save()}
            disabled={saving || !key.trim() || !value.trim()}
          >
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            className={ICON_BUTTON_CLASS_NAME}
            title="Cancel"
            aria-label="Cancel new variable"
            onClick={onDone}
            disabled={saving}
          >
            <X size={13} />
          </button>
        </td>
      </tr>
      {key.trim() !== "HF_TOKEN" && HF_TOKEN_RE.test(value.trim()) && <HfHintRow />}
    </>
  );
}

function EnvVarsSection() {
  const [vars, setVars] = useState<EnvVar[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getEnvVars()
      .then(setVars)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  // Every mutation returns the fresh full list; success clears a stale error.
  const applyVars = (v: EnvVar[]) => {
    setVars(v);
    setError(null);
  };

  // Recommended keys first (fixed order), then custom variables in file order.
  const customKeys =
    vars === null ? [] : vars.map((v) => v.key).filter((k) => !RECOMMENDED_ENV_KEYS.includes(k));
  const names = [...RECOMMENDED_ENV_KEYS, ...customKeys];

  return (
    <div className={SETTINGS_CARD_CLASS_NAME}>
      <div className="settings-card-head flex items-center gap-2.5 mb-3">
        <h3>Environment variables</h3>
        <div className="spacer" style={{ flex: 1 }} />
        <button
          className={SMALL_BUTTON_CLASS_NAME}
          onClick={() => setAdding(true)}
          disabled={adding || vars === null}
        >
          <Plus size={12} /> Add variable
        </button>
      </div>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Stored in <code>~/.openresearch/env</code> and passed to runs and the research agent.{" "}
        <code>HF_TOKEN</code> and <code>WANDB_API_KEY</code> are always listed since runs
        typically need them. Variables set in orx's own environment win on conflicts.
      </p>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : vars === null ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading…
        </div>
      ) : (
        <table className="env-table w-full border-collapse text-md table-fixed [&_td:first-child]:w-[32%] [&_td:first-child]:wrap-anywhere [&_.badge]:ml-2 [&_input]:w-full [&_input]:border-0 [&_input]:bg-transparent [&_input]:p-0 [&_input:focus]:shadow-[0_1px_0_0_var(--text)] [&_td]:h-9 [&_td]:pt-0 [&_td]:pr-2.5 [&_td]:pb-0 [&_td]:pl-0 [&_td]:align-middle [&_td]:border-b [&_td]:border-b-border-variant [&_td:last-child]:w-29 [&_td:last-child]:whitespace-nowrap [&_td:last-child]:text-right [&_td[colspan]]:whitespace-normal [&_td[colspan]]:text-left [&_.icon-btn]:ml-2 [&_.icon-btn]:align-middle [&_.icon-btn:hover]:text-accent-red">
          <tbody>
            {names.map((name) => (
              <EnvRow
                key={name}
                name={name}
                entry={vars.find((v) => v.key === name)}
                onVars={applyVars}
                onError={setError}
              />
            ))}
            {adding && (
              // onDone deliberately leaves the error slot alone — cancelling
              // the add row must not wipe another row's failure message.
              <AddVarRow onVars={applyVars} onError={setError} onDone={() => setAdding(false)} />
            )}
          </tbody>
        </table>
      )}
      {error && <div className="error">{error}</div>}
    </div>
  );
}

// --- appearance ----------------------------------------------------------------

const THEME_OPTIONS: {
  value: ThemePreference;
  label: string;
  icon: typeof Monitor;
}[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

function AppearanceTab() {
  const [preference, setPreference] = useThemePreference();

  // Arrow keys move selection relative to the focused radio, with focus
  // following the new choice (WAI-ARIA radio pattern).
  const onKeyDown = (e: React.KeyboardEvent) => {
    const dir =
      e.key === "ArrowRight" || e.key === "ArrowDown"
        ? 1
        : e.key === "ArrowLeft" || e.key === "ArrowUp"
          ? -1
          : 0;
    if (!dir) return;
    e.preventDefault();
    const radios = [
      ...e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]'),
    ];
    const from = radios.findIndex((r) => r === document.activeElement);
    const anchor =
      from === -1 ? THEME_OPTIONS.findIndex((o) => o.value === preference) : from;
    const next = (anchor + dir + THEME_OPTIONS.length) % THEME_OPTIONS.length;
    setPreference(THEME_OPTIONS[next].value);
    radios[next]?.focus();
  };

  return (
    <>
      <h2>Appearance</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">How the interface looks on this device.</p>
      <div className={SETTINGS_CARD_CLASS_NAME}>
        <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
          <div>
            <div className="project-default-title text-md font-semibold">Theme</div>
            <p>System follows your operating system's light or dark setting.</p>
          </div>
          <div
            className="theme-segmented inline-flex flex-none gap-0.5 p-0.5 border border-border rounded-md bg-surface"
            role="radiogroup"
            aria-label="Theme"
            onKeyDown={onKeyDown}
          >
            {THEME_OPTIONS.map(({ value, label, icon: Icon }) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={preference === value}
                tabIndex={preference === value ? 0 : -1}
                className={`theme-segment inline-flex items-center gap-1.5 py-[5px] px-2.5 rounded-sm text-subtext text-sm cursor-pointer transition-[background,color] duration-120 ease-standard [&:hover:not(.on)]:text-text [&:hover:not(.on)]:bg-highlight [&.on]:text-background [&.on]:bg-primary [&:focus-visible]:outline-2 [&:focus-visible]:outline-solid [&:focus-visible]:outline-text [&:focus-visible]:outline-offset-2 ${preference === value ? "on" : ""}`}
                onClick={() => setPreference(value)}
              >
                <Icon size={14} />
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>
    </>
  );
}

// --- updates -----------------------------------------------------------------

const CHANNEL_LABELS: Record<InstallChannel, string> = {
  installer: "Installed with the orx installer",
  "app-bundle": "macOS app",
  cargo: "Installed with cargo",
  homebrew: "Installed with Homebrew",
  nix: "Managed by Nix",
  unknown: "Unknown install",
};

/** What to do about updates when orx can't do them itself. */
const MANUAL_UPDATE_HINT: Partial<Record<InstallChannel, string>> = {
  cargo: "Re-run your cargo install to update.",
  homebrew: "Run brew upgrade to update.",
  nix: "Update it through your Nix configuration.",
};

function UpdatesTab() {
  const { status, error: loadError, apply } = useUpdateStatus();
  // Per-action only so the right button reads "Working…"; any write disables
  // all three, since they mutate overlapping state.
  const [busy, setBusy] = useState<"auto" | "apply" | "cli" | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!status) {
    return (
      <>
        <h2>Updates</h2>
        {loadError ? (
          <div className={SETTINGS_CARD_CLASS_NAME}>
            <div className="error">{loadError}</div>
          </div>
        ) : (
          <div className={SETTINGS_LOADING_CLASS_NAME}>
            <span className={SPINNER_CLASS_NAME} /> Loading…
          </div>
        )}
      </>
    );
  }

  // Every mutating call returns the authoritative status; adopting it is what
  // keeps the switch honest when a write fails or another client changes it.
  const run = async (which: "auto" | "apply" | "cli", action: () => Promise<unknown>) => {
    setBusy(which);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <h2>Updates</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Keeping this copy of orx on the latest release.
      </p>
      <div className={SETTINGS_CARD_CLASS_NAME}>
        <div className={KV_CLASS_NAME}>
          <div className="k">Version</div>
          <div className="v">{status.current}</div>
          <div className="k">Latest</div>
          <div className="v">{status.latest ?? "—"}</div>
          <div className="k">Install</div>
          <div className="v">{CHANNEL_LABELS[status.channel]}</div>
        </div>

        {status.restartRequired && (
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-md font-semibold">
                Restart to finish updating
              </div>
              <p>
                Version {status.installedVersion} is installed. This window is still running{" "}
                {status.current}.
              </p>
            </div>
          </div>
        )}

        {status.selfUpdates ? (
          <>
            <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
              <div>
                <div className="project-default-title text-md font-semibold">
                  Install updates automatically
                </div>
                <p>
                  New releases are downloaded and installed in the background. Turning this off
                  keeps the notice but leaves the install to you.
                  {status.envDisabled &&
                    " Updates are switched off for this environment by ORX_NO_UPDATE_CHECK, so this setting has no effect."}
                </p>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={status.autoUpdate}
                aria-label="Install updates automatically"
                className={`${SETTINGS_SWITCH_CLASS_NAME} ${status.autoUpdate ? "on" : ""}`}
                disabled={busy !== null}
                onClick={() =>
                  void run("auto", () => setAutoUpdateApi(!status.autoUpdate).then(apply))
                }
              >
                <span />
              </button>
            </div>
            <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
              <div>
                <div className="project-default-title text-md font-semibold">
                  {status.updateAvailable ? `Update to ${status.latest}` : "Check for updates"}
                </div>
                <p>
                  {status.updateAvailable
                    ? "Install the new release now instead of waiting for the background update."
                    : "orx checks a few times a day on its own."}
                </p>
              </div>
              <button
                type="button"
                className={SMALL_BUTTON_CLASS_NAME}
                disabled={busy !== null}
                onClick={() => void run("apply", () => applyUpdate().then(apply))}
              >
                {busy === "apply"
                  ? "Working…"
                  : status.updateAvailable
                    ? "Update now"
                    : "Check now"}
              </button>
            </div>
          </>
        ) : (
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-md font-semibold">
                orx can't update this install
              </div>
              <p>
                {MANUAL_UPDATE_HINT[status.channel] ??
                  "Reinstall with the orx installer to get automatic updates."}
              </p>
            </div>
          </div>
        )}

        {status.channel === "app-bundle" && <InstallCliRow busy={busy} run={run} />}
        {error && <div className="error">{error}</div>}
      </div>
    </>
  );
}

/** Offered only inside the macOS app: the bundle carries an `orx` its owner's
 *  terminal can't see until it's linked onto PATH. */
function InstallCliRow({
  busy,
  run,
}: {
  busy: "auto" | "apply" | "cli" | null;
  run: (which: "cli", action: () => Promise<unknown>) => Promise<void>;
}) {
  const [result, setResult] = useState<InstalledCli | null>(null);
  // Set once a plain install was refused for an existing orx on PATH; the retry
  // is what makes the backend's "re-run with --force" reachable from here.
  const [needsForce, setNeedsForce] = useState(false);

  const install = (force: boolean) =>
    void run("cli", () =>
      installCli(force)
        .then((r) => {
          setResult(r);
          setNeedsForce(false);
        })
        .catch((e) => {
          // Only the PATH-collision refusal is retryable with force; an
          // unrelated failure must not relabel the button "Replace anyway".
          setNeedsForce(!force && String(e?.message ?? e).includes("--force"));
          throw e;
        }),
    );

  return (
    <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
      <div>
        <div className="project-default-title text-md font-semibold">
          Install the <code>orx</code> command
        </div>
        {result ? (
          <p>
            {result.alreadyCurrent ? "Already linked at " : "Linked "}
            <code>{result.link}</code>.
            {!result.onPath && ` Add ${result.dir} to your PATH to use it.`}
          </p>
        ) : (
          <p>
            Adds <code>orx</code> to your terminal, pointing at this app — so the CLI and the app
            are always the same version.
          </p>
        )}
      </div>
      <button
        type="button"
        className={SMALL_BUTTON_CLASS_NAME}
        disabled={busy !== null}
        onClick={() => install(needsForce)}
      >
        {busy === "cli" ? "Working…" : needsForce ? "Replace anyway" : result ? "Re-link" : "Install"}
      </button>
    </div>
  );
}

// --- project defaults ----------------------------------------------------------

function ProjectDefaultsTab() {
  const [settings, setSettings] = useState<ProjectDefaultsSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    setError(null);
    void getProjectDefaults()
      .then(setSettings)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };
  useEffect(load, []);

  const toggle = () => {
    if (!settings || saving) return;
    const enabled = !settings.githubForNewProjects;
    setSaving(true);
    setError(null);
    void setProjectDefaults(enabled, true)
      .then(setSettings)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setSaving(false));
  };

  return (
    <>
      <h2>General</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">Defaults applied when you create a project.</p>
      {!settings ? (
        error ? <div className="error">{error}</div> : <div className={SETTINGS_LOADING_CLASS_NAME}><span className={SPINNER_CLASS_NAME} /> Loading…</div>
      ) : (
        <div className="settings-card [&_>_.error]:text-accent-red [&_>_.error]:text-md [&_>_.error]:whitespace-pre-wrap bg-background border border-border rounded-lg py-4 px-4.5 mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5 [&_h3]:text-sm [&_h3]:font-semibold [&_h3]:text-text [&_.settings-sub]:mb-3 [&_.kv]:gap-y-1.5 [&_.kv]:gap-x-4.5 [&_>_.project-default-row:first-child]:pt-0 [&_>_.project-default-row:first-child]:border-t-0 project-defaults-card [&_.settings-card-head]:justify-between [&_.settings-card-head]:mb-0 [&_.settings-card-head]:pb-3 [&_.settings-card-head_h3]:m-0">
          <div className="settings-card-head flex items-center gap-2.5 mb-3">
            <h3>GitHub publishing</h3>
            <span className={`${BADGE_CLASS_NAME} ${settings.githubAuthenticated ? "ok" : ""}`}>
              {settings.githubAuthenticated ? `Connected via ${settings.githubTokenSource}` : "Not connected"}
            </span>
          </div>
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-md font-semibold">Enable GitHub syncing for new projects</div>
              <p>
                When enabled, each new project gets a private GitHub repository. Experiment
                branches are pushed automatically for collaborator visibility. Compute always
                uses direct source snapshots.
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={settings.githubForNewProjects}
              aria-label="Enable GitHub syncing for new projects"
              className={`${SETTINGS_SWITCH_CLASS_NAME} ${settings.githubForNewProjects ? "on" : ""}`}
              disabled={saving || (!settings.githubAuthenticated && !settings.githubForNewProjects)}
              onClick={toggle}
            >
              <span />
            </button>
          </div>
          {!settings.githubAuthenticated && (
            <div className="project-default-connect [&_p]:mt-[3px] [&_p]:mx-0 [&_p]:mb-0 [&_p]:text-muted [&_p]:text-sm mt-3.5 pt-3.5 border-t border-t-border-variant [&_.onb-token-form]:mt-2.5">
              <p>Connect GitHub to make publishing the default for new projects.</p>
              <GitTokenForm onSaved={load} />
            </div>
          )}
          {error && <div className="error">{error}</div>}
        </div>
      )}
    </>
  );
}

// --- git -----------------------------------------------------------------------

function GitTab({
  project,
  publicationError,
  onProjectUpdate,
}: {
  project: Project | null;
  publicationError: string | null;
  onProjectUpdate: (project: Project) => void;
}) {
  const [status, setStatus] = useState<ProjectGitStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [defaultPromptOpen, setDefaultPromptOpen] = useState(false);
  const [defaultPromptSaving, setDefaultPromptSaving] = useState(false);
  const [defaultPromptError, setDefaultPromptError] = useState<string | null>(null);
  const seqRef = useRef(0);
  const hasGithubRepository = Boolean(status?.github.owner && status.github.repo);

  const load = () => {
    const request = ++seqRef.current;
    setStatus(null);
    setError(null);
    if (!project) return;
    void getProjectGitStatus(project.id)
      .then((projectStatus) => {
        if (request !== seqRef.current) return;
        setStatus(projectStatus);
      })
      .catch((err) => {
        if (request === seqRef.current) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });
  };
  useEffect(load, [project?.id]);

  const syncErrorMessage = (err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    if (message.toLowerCase().includes("archived")) {
      return "GitHub rejected the push because this repository is archived and read-only. The local project is still available. Unarchive the repository on GitHub, then enable syncing here.";
    }
    if (message.includes("(fetch first)") || message.includes("non-fast-forward")) {
      return "GitHub contains changes that are not in this local project. Pull the latest GitHub changes and resolve any conflicts in Git, then try enabling syncing again.";
    }
    if (message.includes("403") || message.toLowerCase().includes("permission denied")) {
      return "GitHub rejected the push. Make sure your connected account has write access to this repository, then try again.";
    }
    return message;
  };

  const enableSync = () => {
    if (!project) return;
    setSaving(true);
    setError(null);
    void enableProjectGithub(project.id)
      .then((result) => {
        setStatus(result.git);
        onProjectUpdate(result.project);
        void getProjectDefaults()
          .then((defaults) => {
            if (!defaults.githubForNewProjects && !defaults.githubDefaultPromptSeen) {
              setDefaultPromptOpen(true);
            }
          })
          .catch(() => {});
      })
      .catch((err) => setError(syncErrorMessage(err)))
      .finally(() => setSaving(false));
  };

  const finishDefaultPrompt = (enabled: boolean) => {
    setDefaultPromptSaving(true);
    setDefaultPromptError(null);
    void setProjectDefaults(enabled, true)
      .then(() => setDefaultPromptOpen(false))
      .catch((err) => setDefaultPromptError(err instanceof Error ? err.message : String(err)))
      .finally(() => setDefaultPromptSaving(false));
  };

  return (
    <>
      <h1>Repository</h1>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Git and GitHub settings for <strong>{project?.name ?? "the current project"}</strong>.
        Local Git powers experiments; publishing is optional.
      </p>
      {!project ? (
        <div className={SETTINGS_CARD_CLASS_NAME}><p className={SETTINGS_NOTE_CLASS_NAME}>Open a project to inspect its repository and GitHub publication state.</p></div>
      ) : error && !status ? (
        <div className="error">{error}</div>
      ) : !status ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading…
        </div>
      ) : (
        <>
          <div className={GIT_SETTINGS_CARD_CLASS_NAME}>
            <h3>Local repository</h3>
            <div className={KV_CLASS_NAME}>
              <span className="k">Path</span><span className={`v ${MONO_CLASS_NAME}`}>{status.path}</span>
              <span className="k">Git</span><span className="v">{status.gitVersion ?? "not found"}</span>
              <span className="k">State</span><span className="v">{status.initialized ? `${status.currentBranch ?? "detached"} · ${status.clean ? "clean" : "has changes"}` : "not initialized"}</span>
              <span className="k">Baseline</span><span className={`v ${MONO_CLASS_NAME}`}>{status.baselineBranch}</span>
              <span className="k">Remotes</span><span className="v">{status.remotes.length ? status.remotes.map((remote) => `${remote.name}: ${remote.url}`).join(" · ") : "none"}</span>
            </div>
            {!status.initialized && <div className={GIT_CARD_ACTIONS_CLASS_NAME}><button className={PRIMARY_BUTTON_CLASS_NAME} onClick={() => void initializeProjectGit(project.id).then(setStatus).catch((err) => setError(String(err)))}>Initialize Git</button></div>}
          </div>
          <div className={GIT_SETTINGS_CARD_CLASS_NAME}>
            <h3>GitHub</h3>
            <div className={KV_CLASS_NAME}>
              <span className="k">Authentication</span><span className="v"><span className={`${BADGE_CLASS_NAME} ${status.github.authenticated ? "ok" : ""}`}>{status.github.authenticated ? "Connected" : "Not connected"}</span>{status.github.authenticated && <span className="git-detail-meta text-muted text-sm">via {status.github.tokenSource}</span>}</span>
              <span className="k">Project</span><span className="v">{hasGithubRepository ? <><span className={MONO_CLASS_NAME}>{status.github.owner}/{status.github.repo}</span>{!status.github.enabled && <span className="badge inline-flex items-center font-sans font-medium py-px px-[7px] border border-border rounded-sm [&.ok]:text-accent-green [&.ok]:border-accent-green [&.ok]:bg-accent-green-subtle [&.err]:text-accent-red [&.err]:border-accent-red [&.err]:bg-accent-red-subtle [&.warn]:text-accent-amber [&.warn]:border-accent-amber [&.warn]:bg-accent-amber-subtle git-detail-meta text-muted text-sm">Syncing off</span>}</> : <span className={BADGE_CLASS_NAME}>Local only</span>}</span>
              {status.github.enabled && <><span className="k">Sync</span><span className="v">{status.github.syncStatus}</span></>}
            </div>
            {!status.github.authenticated && (
              <>
                <p className="git-card-helper text-muted text-sm mt-3.5 mx-0 mb-0">
                  GitHub is optional. Connect only when you want a hosted copy for collaboration.
                </p>
                <GitTokenForm onSaved={() => load()} />
              </>
            )}
            {status.github.authenticated && !status.github.enabled && (
              <>
                <p className="git-card-helper text-muted text-sm mt-3.5 mx-0 mb-0">
                  {hasGithubRepository
                    ? "Use this repository for automatic experiment-branch pushes when your connected account can write to it. Otherwise, OpenResearch creates a separate private repository for collaboration."
                    : "Create a private repository and automatically push experiment branches for collaborator visibility."}
                </p>
                <div className={GIT_CARD_ACTIONS_CLASS_NAME}>
                  {hasGithubRepository && status.github.url && <a className={BUTTON_CLASS_NAME} href={status.github.url} target="_blank" rel="noreferrer">Open on GitHub <ExternalLink size={12} /></a>}
                  <button className={PRIMARY_BUTTON_CLASS_NAME} disabled={saving} onClick={enableSync}>{saving ? "Enabling…" : "Enable GitHub syncing"}</button>
                </div>
              </>
            )}
            {status.github.enabled && (
              <>
                <p className="git-card-helper text-muted text-sm mt-3.5 mx-0 mb-0">
                  Disabling syncing stops automatic pushes. Compute continues to use direct source
                  snapshots. This does not delete the GitHub repository or code already pushed.
                </p>
                <div className={GIT_CARD_ACTIONS_CLASS_NAME}>
                  {status.github.url && <a className={BUTTON_CLASS_NAME} href={status.github.url} target="_blank" rel="noreferrer">Open on GitHub <ExternalLink size={12} /></a>}
                  <button className={BUTTON_CLASS_NAME} disabled={saving} onClick={() => { setSaving(true); void disableProjectGithub(project.id).then((result) => { setStatus(result.git); onProjectUpdate(result.project); }).catch((err) => setError(err instanceof Error ? err.message : String(err))).finally(() => setSaving(false)); }}>{saving ? "Updating…" : "Disable syncing"}</button>
                </div>
              </>
            )}
          </div>
          {publicationError && <div className="error">{syncErrorMessage(publicationError)}</div>}
          {error && <div className="error">{syncErrorMessage(error)}</div>}
        </>
      )}
      {defaultPromptOpen && (
        <div className="modal-backdrop fixed inset-0 bg-[rgba(29,_27,_26,_0.4)] flex items-start justify-center pt-[var(--modal-top)] px-4 pb-6 overflow-y-auto z-100" onClick={() => finishDefaultPrompt(false)}>
          <div
            className="modal max-w-[94vw] max-h-[calc(100vh_-_var(--modal-top)_-_48px)] overflow-y-auto bg-background border border-border rounded-xl shadow-[0_24px_60px_rgba(0,_0,_0,_0.22)] p-6 [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-xl github-default-modal w-110 [&_>_p]:m-0 [&_>_p]:text-muted [&_>_p]:text-md [&_>_p]:leading-normal [&_>_.error]:mt-3.5"
            role="dialog"
            aria-modal="true"
            aria-labelledby="github-default-title"
            onClick={(event) => event.stopPropagation()}
          >
            <h2 id="github-default-title">Make GitHub syncing the default?</h2>
            <p>
              This is useful when collaborators follow project changes on GitHub. New projects
              will enable syncing automatically, creating a private repository when needed and
              pushing experiment branches for visibility.
            </p>
            {defaultPromptError && <div className="error">{defaultPromptError}</div>}
            <div className="github-default-actions flex justify-end gap-2.5 mt-5.5">
              <button className={BUTTON_CLASS_NAME} disabled={defaultPromptSaving} onClick={() => finishDefaultPrompt(false)}>
                Not now
              </button>
              <button className={PRIMARY_BUTTON_CLASS_NAME} disabled={defaultPromptSaving} onClick={() => finishDefaultPrompt(true)}>
                {defaultPromptSaving ? "Saving…" : "Make default"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

// --- storage (data directory) ------------------------------------------------

const DATA_DIR_SOURCE_LABEL: Record<DataDirSettings["source"], string> = {
  env: "ORX_DATA_DIR environment variable",
  config: "your saved setting",
  xdg: "XDG_DATA_HOME",
  default: "default location",
};

type MoveState =
  | { kind: "idle" }
  | { kind: "moving"; phase: string; copied: number; total: number }
  | { kind: "done"; oldPathLeft?: string }
  | { kind: "error"; message: string };

function StorageTab() {
  const [settings, setSettings] = useState<DataDirSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [checking, setChecking] = useState(false);
  const [validation, setValidation] = useState<DataDirValidation | null>(null);
  const [move, setMove] = useState<MoveState>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    getDataDir()
      .then((s) => {
        setSettings(s);
        // Seed the input to the current path only when empty — preserves an
        // in-progress edit, and (after a move clears it) re-seeds to the new path.
        setPath((p) => (p ? p : s.current));
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));

  useEffect(() => {
    void load();
  }, []);

  // Subscribe to move progress streamed over the shared SSE.
  useEffect(() => {
    return onDataDirMove((ev) => {
      if (ev.type === "progress") {
        // The "preparing" tick reports total 0 (sized after the checkpoint);
        // keep the last known non-zero total so the bar doesn't flicker to 0.
        setMove((m) => {
          const prevTotal = m.kind === "moving" ? m.total : 0;
          return {
            kind: "moving",
            phase: ev.phase,
            copied: ev.copiedBytes,
            total: ev.totalBytes || prevTotal,
          };
        });
      } else if (ev.type === "done") {
        setMove({ kind: "done", oldPathLeft: ev.oldPathLeft });
        setValidation(null);
        // Clear so load()'s empty-guard re-seeds the input to the new path.
        setPath("");
        void load();
      } else if (ev.type === "error") {
        setMove({ kind: "error", message: ev.error });
      }
    });
  }, []);

  const envForced = settings?.source === "env";
  const trimmed = path.trim();
  const unchanged = settings !== null && trimmed === settings.current;

  async function check() {
    if (checking || !trimmed) return;
    setChecking(true);
    setError(null);
    setValidation(null);
    try {
      setValidation(await validateDataDir(trimmed));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  }

  async function startMove(e: React.FormEvent) {
    e.preventDefault();
    if (move.kind === "moving" || !trimmed || unchanged) return;
    setError(null);
    // Confirm — this relocates all projects' data. Same-disk moves are atomic;
    // cross-disk moves copy and leave the old folder for you to remove.
    if (
      !window.confirm(
        `Move all orx data to:\n${trimmed}\n\nThe store is copied to the new location and ` +
          `activated there. Active runs or chats will block the move.`,
      )
    )
      return;
    setMove({ kind: "moving", phase: "preparing", copied: 0, total: validation?.treeBytes ?? 0 });
    try {
      await moveDataDir(trimmed);
      // 202 accepted — progress/done arrive over SSE. Nothing else to do here.
    } catch (err) {
      // 409 in-flight guard or a validation error surfaces here.
      setMove({ kind: "idle" });
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <>
      <h2>Storage</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
        Where orx keeps everything on this machine — the local database, run logs, artifacts, and
        chat attachments for <strong>all</strong> projects. Moving it copies the whole store to the
        new location and activates it there.
      </p>
      {loadError ? (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="error">{loadError}</div>
        </div>
      ) : !settings ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading…
        </div>
      ) : (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="settings-card-head flex items-center gap-2.5 mb-3">
            <h3>Data directory</h3>
            <div className="spacer" style={{ flex: 1 }} />
            <span className={BADGE_CLASS_NAME}>{settings.isDefault ? "Default" : "Custom"}</span>
          </div>
          <div className={KV_CLASS_NAME}>
            <span className="k">Current</span>
            <span className={`v ${MONO_CLASS_NAME}`}>{settings.current}</span>
            <span className="k">Source</span>
            <span className="v">{DATA_DIR_SOURCE_LABEL[settings.source]}</span>
            {!settings.isDefault && (
              <>
                <span className="k">Default</span>
                <span className={`v ${MONO_CLASS_NAME}`}>{settings.defaultPath}</span>
              </>
            )}
          </div>

          {envForced ? (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              The data directory is pinned by the <code>ORX_DATA_DIR</code> environment variable,
              which overrides this setting. Unset it to choose a location here.
            </p>
          ) : (
            <form className={FORM_CLASS_NAME} onSubmit={startMove}>
              <label>
                New location
                <input
                  className={MONO_CLASS_NAME}
                  type="text"
                  value={path}
                  onChange={(e) => {
                    setPath(e.target.value);
                    setValidation(null);
                  }}
                  placeholder="/absolute/path/to/openresearch"
                  autoComplete="off"
                  spellCheck={false}
                  disabled={move.kind === "moving"}
                />
              </label>

              {validation && !validation.error && validation.ok && (
                <p className={SETTINGS_NOTE_CLASS_NAME}>
                  Ready to move {fmtBytes(validation.treeBytes ?? 0)}
                  {validation.freeBytes != null && ` — ${fmtBytes(validation.freeBytes)} free at target`}
                  {validation.sameFilesystem ? " (same disk, instant)" : ""}.
                </p>
              )}
              {validation && validation.ok === false && validation.error && (
                <div className="error">{validation.error}</div>
              )}
              {error && <div className="error">{error}</div>}

              {move.kind === "moving" && (
                <ProgressBar
                  value={move.copied}
                  max={move.total}
                  label={`${move.phase.charAt(0).toUpperCase()}${move.phase.slice(1)}…`}
                  caption={
                    move.total > 0 ? (
                      <span className={MONO_CLASS_NAME}>
                        {fmtBytes(move.copied)} / {fmtBytes(move.total)}
                      </span>
                    ) : undefined
                  }
                />
              )}
              {move.kind === "done" && (
                <p className={SETTINGS_NOTE_CLASS_NAME}>
                  Moved. orx is now using the new location.
                  {move.oldPathLeft && (
                    <>
                      {" "}
                      The old copy was left at <code>{move.oldPathLeft}</code> (different disk) — you
                      can delete it once you&apos;ve confirmed everything works.
                    </>
                  )}
                </p>
              )}
              {move.kind === "error" && <div className="error">Move failed: {move.message}</div>}

              <div className="actions">
                <button
                  type="button"
                  className={BUTTON_CLASS_NAME}
                  onClick={check}
                  disabled={checking || !trimmed || unchanged || move.kind === "moving"}
                >
                  {checking ? "Checking…" : "Check"}
                </button>
                <button
                  type="submit"
                  className={PRIMARY_BUTTON_CLASS_NAME}
                  disabled={!trimmed || unchanged || move.kind === "moving"}
                >
                  {move.kind === "moving" ? "Moving…" : "Move data here"}
                </button>
              </div>
            </form>
          )}
        </div>
      )}
    </>
  );
}

// --- instances ---------------------------------------------------------------

const isLive = (status: string) => status === "running" || status === "starting";

/** Runtime: live instances show elapsed-so-far, finished ones total duration.
 *  Both start at submission time, so provisioning/queue time is included —
 *  that's the span the provider bills for. */
function runtimeLabel(inst: Instance): string {
  if (isLive(inst.status)) return fmtDuration(Date.now() - inst.createdAt);
  if (inst.endedAt) return fmtDuration(inst.endedAt - inst.createdAt);
  return "—";
}

/** One section's table: backend (logo + flavor), project, status, started, runtime. */
function InstancesTable({ instances, emptyLabel }: { instances: Instance[]; emptyLabel: string }) {
  if (instances.length === 0) {
    return <p className="instances-empty m-0 py-3.5 px-4 border border-border rounded-lg bg-background text-subtext text-md">{emptyLabel}</p>;
  }
  return (
    <div className="instances-table-wrap overflow-x-auto">
      <table className="runs-table w-full border-collapse text-md bg-background [&_th]:text-left [&_th]:text-text [&_th]:text-xs [&_th]:font-semibold [&_th]:py-2 [&_th]:px-3 [&_th]:border-b [&_th]:border-b-border [&_th]:sticky [&_th]:top-0 [&_th]:bg-background [&_th]:z-1 [&_td]:py-2 [&_td]:px-3 [&_td]:border-b [&_td]:border-b-[color-mix(in_oklab,_var(--text)_6%,_transparent)] [&_td]:whitespace-nowrap [&_tr:last-child_td]:border-b-0 [&_tr.clickable]:cursor-pointer [&_tr.clickable:hover_td]:bg-canvas">
        <thead>
          <tr>
            <th>Backend</th>
            <th>Project</th>
            <th>Status</th>
            <th>Started</th>
            <th>Runtime</th>
          </tr>
        </thead>
        <tbody>
          {instances.map((inst) => {
            // HF jobs carry their dashboard URL; Modal stores only a sandbox id.
            const url = typeof inst.backend?.url === "string" ? inst.backend.url : undefined;
            return (
              <tr key={inst.id}>
                <td>
                  <span className="backend-cell inline-flex items-center gap-0.5 [&_.icon-btn]:w-5.5 [&_.icon-btn]:h-5.5">
                    <BackendBadge backend={inst.backend} />
                    {url && (
                      <a
                        className={ICON_BUTTON_CLASS_NAME}
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        title="Open job page"
                        aria-label="Open job page"
                        onClick={(e) => e.stopPropagation()}
                      >
                        <ExternalLink size={12} />
                      </a>
                    )}
                  </span>
                </td>
                <td>{inst.projectName ?? shortId(inst.projectId)}</td>
                <td>
                  <StatusBadge status={runDisplayStatus(inst)} />
                </td>
                <td>{timeAgo(inst.createdAt)}</td>
                <td>{runtimeLabel(inst)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function ComputeActivity({ onViewHistory }: { onViewHistory: () => void }) {
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Re-render every 30s so live rows' Runtime keeps counting (client-side
  // only — the minute-level display doesn't warrant a refetch).
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, []);

  // Point-in-time snapshot: the page refetches on every open, and this button
  // refreshes in place while sitting on it — the run.updated
  // SSE stream carries no projectName, so it can't drive this list directly.
  const load = () => {
    setRefreshing(true);
    listInstances()
      .then((rows) => {
        setInstances(rows);
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setInstances((prev) => prev ?? []);
      })
      .finally(() => setRefreshing(false));
  };
  useEffect(() => load(), []);

  const byRecent = (a: Instance, b: Instance) => b.createdAt - a.createdAt;
  const running = instances?.filter((i) => isLive(i.status)).sort(byRecent);
  const past = instances?.filter((i) => !isLive(i.status)).sort(byRecent);

  return (
    <section className="compute-activity [&_.count-badge]:inline-flex [&_.count-badge]:items-center [&_.count-badge]:justify-center [&_.count-badge]:min-w-4.5 [&_.count-badge]:h-4.5 [&_.count-badge]:py-0 [&_.count-badge]:px-[5px] [&_.count-badge]:rounded-md [&_.count-badge]:bg-canvas [&_.count-badge]:border [&_.count-badge]:border-border [&_.count-badge]:text-xs [&_.count-badge]:font-medium [&_.count-badge]:text-text mt-5.5 mx-0 mb-6.5">
      <div className="compute-activity-head flex items-start justify-between gap-5 mb-3.5 [&_h2]:flex [&_h2]:items-center [&_h2]:gap-2 [&_h2]:m-0 [&_h2]:text-lg [&_p]:mt-[3px] [&_p]:mx-0 [&_p]:mb-0 [&_p]:text-muted [&_p]:text-sm [@media((max-width:_640px))]:items-stretch [@media((max-width:_640px))]:flex-col">
        <div>
          <h2>
            Running instances
            {running && running.length > 0 && <span className="count-badge">{running.length}</span>}
          </h2>
          <p>Compute currently active across all projects.</p>
        </div>
        <div className="compute-activity-actions flex gap-2 flex-none [@media((max-width:_640px))]:justify-start">
          <button className={SMALL_BUTTON_CLASS_NAME} onClick={load} disabled={refreshing}>
            <RefreshCw size={12} className={refreshing ? "spin animate-[settings-spin_0.9s_linear_infinite]" : ""} /> Refresh
          </button>
          <button className={SMALL_BUTTON_CLASS_NAME} onClick={onViewHistory}>
            {`View history${past?.length ? ` (${past.length})` : ""}`}
          </button>
        </div>
      </div>
      {error && <div className="error">{error}</div>}
      {!running || !past ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}>
          <span className={SPINNER_CLASS_NAME} /> Loading…
        </div>
      ) : <InstancesTable instances={running} emptyLabel="Nothing running right now." />}
    </section>
  );
}

function InstanceHistory({ onBack }: { onBack: () => void }) {
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [, setTick] = useState(0);

  useEffect(() => {
    const timer = setInterval(() => setTick((tick) => tick + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const load = () => {
    setRefreshing(true);
    listInstances()
      .then((rows) => {
        setInstances(rows.sort((a, b) => b.createdAt - a.createdAt));
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setInstances((current) => current ?? []);
      })
      .finally(() => setRefreshing(false));
  };
  useEffect(load, []);

  return (
    <>
      <button type="button" className="settings-back inline-flex items-center gap-1.5 mt-0 mx-0 mb-4.5 text-subtext text-sm font-medium [&:hover]:text-text" onClick={onBack}>
        <ArrowLeft size={14} /> Back to Compute
      </button>
      <div className="settings-head-row flex items-center justify-between gap-2.5 [&_h1]:m-0">
        <h1>Instance history</h1>
        <button className={SMALL_BUTTON_CLASS_NAME} onClick={load} disabled={refreshing}>
          <RefreshCw size={12} className={refreshing ? "spin animate-[settings-spin_0.9s_linear_infinite]" : ""} /> Refresh
        </button>
      </div>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">Every compute instance spun up across your projects.</p>
      {error && <div className="error">{error}</div>}
      {!instances ? (
        <div className={SETTINGS_LOADING_CLASS_NAME}><span className={SPINNER_CLASS_NAME} /> Loading…</div>
      ) : (
        <InstancesTable instances={instances} emptyLabel="No instances yet." />
      )}
    </>
  );
}

// --- embedded view -----------------------------------------------------------

type SettingsNavItem = {
  id: Tab;
  label: string;
  icon: React.ReactNode;
  activeTabs: Tab[];
};

const SETTINGS_SECTIONS: Tab[] = ["projects", "harnesses", "storage"];

/** Primary rail entries. Configuration sections share the Settings entry. */
export const SETTINGS_NAV: SettingsNavItem[] = [
  {
    id: "compute",
    label: "Compute",
    icon: <Cpu size={15} />,
    activeTabs: ["compute", "instances"],
  },
  { id: "environment", label: "Environment", icon: <SquareTerminal size={15} />, activeTabs: ["environment"] },
  {
    id: "settings",
    label: "Settings",
    icon: <Settings size={15} />,
    activeTabs: ["settings", ...SETTINGS_SECTIONS],
  },
];

function isSettingsSection(tab: Tab): boolean {
  return SETTINGS_SECTIONS.includes(tab);
}

/** One settings section's content, shown in the middle pane in place of chat. */
export function SettingsView({
  tab,
  project,
  githubPublicationError,
  onProjectUpdate,
  onSelectTab,
}: {
  tab: Tab;
  project: Project | null;
  githubPublicationError: string | null;
  onProjectUpdate: (project: Project) => void;
  onSelectTab: (tab: Tab) => void;
}) {
  const showsSettings = tab === "settings" || isSettingsSection(tab);

  return (
    <div className="settings-view max-w-readable my-0 mx-auto pt-6 px-8 pb-15 [&_h1]:mt-0 [&_h1]:mx-0 [&_h1]:mb-1.5 [&_h1]:text-3xl [&_>_.error]:text-accent-red [&_>_.error]:text-md [&_>_.error]:whitespace-pre-wrap [&_>_.error]:mt-0 [&_>_.error]:mx-0 [&_>_.error]:mb-3">
      {showsSettings && (
        <>
          <h1>Settings</h1>
          <div className="settings-stack mt-4.5">
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <AppearanceTab />
            </section>
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <ProjectDefaultsTab />
            </section>
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <HarnessesTab />
            </section>
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <StorageTab />
            </section>
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <UpdatesTab />
            </section>
          </div>
        </>
      )}
      {tab === "compute" && (
        <ComputeTab
          project={project}
          onViewHistory={() => onSelectTab("instances")}
        />
      )}
      {tab === "instances" && <InstanceHistory onBack={() => onSelectTab("compute")} />}
      {tab === "environment" && (
        <>
          <h1>Environment</h1>
          <p className="settings-sub mt-0 mx-0 mb-4.5 text-text text-md">
            Variables available to runs and the research agent (API keys, tokens).
          </p>
          <EnvVarsSection />
        </>
      )}
      {tab === "git" && (
        <GitTab
          project={project}
          publicationError={githubPublicationError}
          onProjectUpdate={onProjectUpdate}
        />
      )}
    </div>
  );
}
