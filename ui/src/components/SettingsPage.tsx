import {
  ArrowLeft,
  ArrowRight,
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
  deleteOverleafToken,
  fmtBytes,
  fmtDuration,
  fmtNumber,
  getComputeSettings,
  getEnvVars,
  getProjectGitStatus,
  getProjectDefaults,
  getTelemetry,
  getHarnesses,
  getHfSettings,
  getK8sSettings,
  getLocalMachine,
  getModalSettings,
  getOpenResearchSettings,
  getOverleafSettings,
  getRaySettings,
  getSlurmSettings,
  getSshHosts,
  getSshMasterStatus,
  listRuns,
  setComputeDefault,
  setProjectDefaults,
  setTelemetry,
  provisionModal,
  saveOverleafToken,
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
  rayPreflight,
  runDisplayStatus,
  timeAgo,
  type ComputeSettings,
  type ComputeTargetId,
  type ComputeTargetSummary,
  type EnvVar,
  type Project,
  type ProjectDefaultsSettings,
  type ProjectGitStatus,
  type TelemetrySettings,
  type Harness,
  type HarnessId,
  type HfSettings,
  type HfTokenSource,
  type K8sSettings,
  type LocalMachine,
  type ModalSettings,
  type ModalTokenSource,
  type OpenResearchSettings,
  type RayPreflight,
  type RaySettings,
  type Run,
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
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { setLocale, useLocale } from "../locale";
import { getLocale, isLocale, type Locale } from "../paraglide/runtime.js";
import { TokenForm } from "./GitTokenForm";
import { renderNote } from "./agentNote";
import { BackendBadge, BackendLogo } from "./BackendLogos";
import { ProgressBar } from "./ProgressBar";
import { OptionPicker } from "./ModelPicker";
import { StatusBadge } from "./StatusBadge";
import { SshConnectTerminal, SshTerminalTranscript } from "./SshConnectTerminal";
import { SshConfigDialog } from "./SshConfigDialog";
import { Badge, Button, ButtonLink, IconButton, IconButtonLink, Input, LoadingRow, showAlert, Spinner, Switch, Tooltip, type BadgeVariant } from "./ui";

const SETTINGS_CARD_CLASS_NAME = [
  "settings-card [&_>_.error]:text-accent-red [&_>_.error]:text-base",
  "[&_>_.error]:whitespace-pre-wrap bg-background border border-border",
  "rounded-lg py-4 px-4.5 mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5",
  "[&_h3]:text-base [&_h3]:font-semibold [&_h3]:text-text",
  "[&_.settings-sub]:mb-3 [&_.kv]:gap-y-1.5 [&_.kv]:gap-x-4.5",
  "[&_>_.project-default-row:first-child]:pt-0 [&_>_.project-default-row:first-child]:border-t-0",
].join(" ");

const KV_CLASS_NAME = [
  "kv grid grid-cols-[auto_1fr] items-baseline gap-y-[3px] gap-x-3.5 text-base",
  "[&_.k]:text-sm [&_.k]:text-subtext [&_.v]:text-base [&_.v]:text-text",
  "[&_.v]:break-all",
].join(" ");

const COMPUTE_DETAILS_CLASS_NAME = [
  "grid grid-cols-[9rem_minmax(0,1fr)] items-center gap-x-5 gap-y-2.5 font-sans text-base text-text",
  "[&_.k]:font-medium [&_.k]:text-sm [&_.k]:text-text",
  "[&_.v]:min-w-0 [&_.v]:flex [&_.v]:items-center [&_.v]:flex-wrap [&_.v]:gap-2",
  "[&_.v]:font-sans [&_.v]:text-base [&_.v]:text-text [&_.v]:break-words",
].join(" ");

const COMPUTE_DIAGNOSTIC_CLASS_NAME =
  "mt-3 mx-0 mb-0 ps-3 border-s-2 border-s-accent-red font-sans text-base leading-relaxed text-text whitespace-pre-wrap";

const SETTINGS_NOTE_CLASS_NAME = [
  "settings-note mt-2.5 mx-0 mb-0 text-base py-2 px-2.5",
  "border border-accent-amber rounded-md bg-accent-amber-subtle",
  "text-accent-amber font-medium",
].join(" ");

const FORM_CLASS_NAME = [
  "form font-sans text-sm text-text [&_.form-seg]:self-start [&_.form-seg]:mb-0.5",
  "[&_.form-seg_button]:py-[5px] [&_.form-seg_button]:px-3",
  "[&_.repo-hint]:font-normal [&_.repo-hint]:text-sm",
  "[&_.repo-hint]:text-muted [&_.repo-hint.ok]:text-accent-teal",
  "[&_.folder-picker-control]:flex [&_.folder-picker-control]:items-center",
  "[&_.folder-picker-control]:gap-[9px] [&_.folder-picker-control]:w-full",
  "[&_.folder-picker-control]:min-w-0 [&_.folder-picker-control]:py-2 [&_.folder-picker-control]:px-2.5",
  "[&_.folder-picker-control]:overflow-hidden [&_.folder-picker-control]:bg-background",
  "[&_.folder-picker-control]:border [&_.folder-picker-control]:border-border",
  "[&_.folder-picker-control]:rounded-md [&_.folder-picker-control]:cursor-pointer",
  "[&_.folder-picker-control]:text-start",
  "[&_.folder-picker-control]:transition-[border-color,box-shadow] [&_.folder-picker-control]:duration-120 [&_.folder-picker-control]:ease-standard",
  "[&_.folder-picker-control:hover:not(:disabled)]:border-muted",
  "[&_.folder-picker-control:hover:not(:disabled)]:shadow-control-subtle",
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
  "[&_.project-location-label]:font-medium [&_.project-field-label]:text-text",
  "[&_.project-field-label]:text-base [&_.project-field-label]:font-medium",
  "[&_.folder-picker-control:disabled]:cursor-default [&_.folder-picker-control:disabled]:opacity-65",
  "[&_.paper-destination]:flex [&_.paper-destination]:items-center",
  "[&_.paper-destination]:gap-2.5 [&_.paper-destination]:pt-2 [&_.paper-destination]:pe-2 [&_.paper-destination]:pb-2 [&_.paper-destination]:ps-3",
  "[&_.paper-destination]:border [&_.paper-destination]:border-border [&_.paper-destination]:rounded-md",
  "[&_.paper-destination]:bg-background [&_.paper-destination_code]:flex-1",
  "[&_.paper-destination_code]:min-w-0 [&_.paper-destination_code]:overflow-hidden",
  "[&_.paper-destination_code]:text-text [&_.paper-destination_code]:text-sm",
  "[&_.paper-destination_code]:font-normal",
  "[&_.paper-destination_code]:text-ellipsis [&_.paper-destination_code]:whitespace-nowrap",
  "[&_.paper-destination_.btn]:flex-none [&_.project-path-notice]:py-[9px] [&_.project-path-notice]:px-[11px]",
  "[&_.project-path-notice]:border [&_.project-path-notice]:border-border-variant",
  "[&_.project-path-notice]:rounded-sm [&_.project-path-notice]:bg-surface",
  "[&_.project-path-notice]:text-base [&_.project-path-notice]:leading-relaxed [&_.project-path-notice]:text-text",
  "[&_.project-path-notice]:leading-[1.4]",
  "[&_.project-path-notice.error]:border-danger-notice-border",
  "[&_.paper-results]:flex [&_.paper-results]:flex-col",
  "[&_.paper-results]:border [&_.paper-results]:border-border [&_.paper-results]:rounded-md",
  "[&_.paper-results]:max-h-60 [&_.paper-results]:overflow-y-auto",
  "[&_.paper-results_button]:flex [&_.paper-results_button]:flex-col",
  "[&_.paper-results_button]:items-start [&_.paper-results_button]:gap-0.5",
  "[&_.paper-results_button]:py-2 [&_.paper-results_button]:px-2.5 [&_.paper-results_button]:bg-none [&_.paper-results_button]:bg-transparent",
  "[&_.paper-results_button]:border-0",
  "[&_.paper-results_button]:border-b [&_.paper-results_button]:border-b-border-variant",
  "[&_.paper-results_button]:text-start [&_.paper-results_button]:[font:inherit]",
  "[&_.paper-results_button]:text-text [&_.paper-results_button]:cursor-pointer",
  "[&_.paper-results_button:last-child]:border-b-0",
  "[&_.paper-results_button:hover]:bg-surface [&_.paper-results_.title]:text-sm",
  "[&_.paper-results_.title]:font-medium",
  "[&_.paper-results_.id]:text-xs [&_.paper-results_.id]:text-muted",
  "[&_.paper-pick_.id]:text-xs",
  "[&_.paper-pick_.id]:text-muted [&_.paper-pick]:flex [&_.paper-pick]:items-center",
  "[&_.paper-pick]:justify-between [&_.paper-pick]:gap-2.5 [&_.paper-pick]:py-2.5 [&_.paper-pick]:px-3",
  "[&_.paper-pick]:border [&_.paper-pick]:border-border [&_.paper-pick]:rounded-md",
  "[&_.paper-pick]:bg-surface [&_.paper-pick_.meta]:min-w-0",
  "[&_.paper-pick_.title]:text-sm [&_.paper-pick_.title]:font-medium",
  "flex flex-col gap-2.5 [&_label]:flex [&_label]:flex-col",
  "[&_label]:gap-1 [&_label]:text-sm [&_label]:text-text",
  "[&_label]:font-medium [&_.row2]:grid [&_.row2]:grid-cols-2",
  "[&_input]:font-sans [&_input]:text-sm [&_input]:font-normal [&_input]:text-text [&_input::placeholder]:text-subtext",
  "[&_select]:font-sans [&_select]:text-sm [&_select]:font-normal [&_select]:text-text",
  "[&_.row2]:gap-2.5 [&_.actions]:flex [&_.actions]:justify-end",
  "[&_.actions]:gap-2.5 [&_.actions]:mt-1.5 [&_.new-project-actions]:justify-start",
  "[&_.new-project-actions]:mt-2.5",
  "[&_.error]:text-accent-red [&_.error]:text-base [&_.error]:whitespace-pre-wrap",
  "settings-form mt-3.5 pt-3.5 border-t border-t-border",
].join(" ");

const PROJECT_DEFAULT_ROW_CLASS_NAME = [
  "project-default-row flex items-center justify-between gap-6",
  "pt-3.5 border-t border-t-border-variant [&_p]:mt-[3px] [&_p]:mx-0 [&_p]:mb-0",
  "[&_.project-default-title]:text-base [&_p]:text-sm [&_p]:leading-relaxed [&_p]:text-text",
].join(" ");

const GIT_SETTINGS_CARD_CLASS_NAME = [
  "settings-card [&_>_.error]:text-accent-red [&_>_.error]:text-base",
  "[&_>_.error]:whitespace-pre-wrap bg-background border border-border",
  "rounded-lg mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5 [&_h3]:text-base",
  "[&_h3]:font-semibold [&_h3]:text-text [&_.settings-sub]:mb-3",
  "[&_>_.project-default-row:first-child]:pt-0 [&_>_.project-default-row:first-child]:border-t-0",
  "git-settings-card py-3.5 px-4 [&_h3]:mb-3",
  "[&_.kv]:grid-cols-[132px_minmax(0,_1fr)] [&_.kv]:items-center [&_.kv]:gap-y-[9px] [&_.kv]:gap-x-4.5",
  "[&_.kv_.k]:text-sm [&_.kv_.v]:flex [&_.kv_.v]:items-center",
  "[&_.kv_.v]:flex-wrap [&_.kv_.v]:gap-[7px] [&_.kv_.v]:min-w-0 [&_.kv_.v]:font-sans",
  "[&_.kv_.v]:text-base [&_.kv_.v]:break-normal",
  "[@media((max-width:_640px))]:[&_.kv]:grid-cols-1",
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

function harnessStatus(h: Harness): { cls: string; variant: BadgeVariant; label: string } {
  if (h.agentReady) return { cls: "ok", variant: "success", label: m.settings_page_signed_in() };
  // Not installed — the same blocker whether or not there's saved auth: the
  // CLI has to be installed before anything can run. Amber "action needed".
  if (!h.installed) return { cls: "warn", variant: "warning", label: m.settings_page_not_installed() };
  if (h.installBroken) return { cls: "warn", variant: "warning", label: m.settings_page_install_broken() };
  if (h.authState === "unknown") return { cls: "warn", variant: "warning", label: m.settings_page_unable_to_verify() };
  if (h.authState === "unsupported") return { cls: "warn", variant: "warning", label: m.settings_page_update_required() };
  return { cls: "warn", variant: "warning", label: m.settings_page_not_signed_in() };
}

function AuthLabel({ h }: { h: Harness }) {
  if (!h.authMethod) return <>—</>;
  return <>{h.authMethod === "oauth" ? m.settings_oauth_login() : m.onboarding_api_key()}</>;
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
      <h2>{m.settings_page_harnesses()}</h2>
      <div className="harness-tabs mt-3 flex gap-1 mb-3.5 border-b border-b-border-variant [&_button]:inline-flex [&_button]:items-center [&_button]:gap-[7px] [&_button]:py-[7px] [&_button]:px-3 [&_button]:text-sm [&_button]:font-medium [&_button]:text-text [&_button]:border-b-2 [&_button]:border-b-transparent [&_button]:-mb-px [&_button:hover]:text-text [&_button.active]:border-b-primary">
        {(harnesses ?? []).map((x) => (
          <button
            key={x.id}
            className={x.id === active ? "active" : ""}
            onClick={() => setActive(x.id)}
          >
            {x.name}
            <span className={`w-[7px] h-[7px] rounded-full bg-muted [&.ok]:bg-accent-green [&.err]:bg-accent-red [&.warn]:bg-accent-amber ${harnessStatus(x).cls}`} />
          </button>
        ))}
      </div>
      {!harnesses ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_detecting_harnesses()}
        </LoadingRow>
      ) : !h ? null : (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="settings-card-head flex items-center gap-2.5 mb-3">
            <Badge variant={harnessStatus(h).variant}>{harnessStatus(h).label}</Badge>
            <div className="spacer flex-1" />
            <Button size="small" onClick={() => load(true, true)} disabled={refreshing}>
              <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
            </Button>
          </div>
          <div className={KV_CLASS_NAME}>
            <span className="k">{m.settings_page_binary()}</span>
            <span className="v">{h.binPath ?? m.settings_not_found_on_path()}</span>
            <span className="k">{m.settings_page_version()}</span>
            <span className="v">{h.version ?? "—"}</span>
            <span className="k">{m.settings_page_auth()}</span>
            <span className="v">
              <AuthLabel h={h} />
            </span>
            {h.account && (
              <>
                <span className="k">{h.id === "opencode" ? m.settings_providers() : m.settings_page_account()}</span>
                <span className="v">{h.account}</span>
              </>
            )}
            {h.org && (
              <>
                <span className="k">{m.settings_page_org()}</span>
                <span className="v">{h.org}</span>
              </>
            )}
            {h.plan && (
              <>
                <span className="k">{m.settings_page_plan()}</span>
                <span className="v">{h.plan}</span>
              </>
            )}
            <span className="k">{m.settings_page_agent_models()}</span>
            <span className="v">
              {h.models.length > 0
                ? m.settings_models_available({ count: fmtNumber(h.models.length), models: new Intl.ListFormat(getLocale()).format(h.models.slice(0, 4).map((model) => ltr(harnessModelLabel(model)))) })
                : m.settings_none()}
            </span>
          </div>
          {h.agentNote && <p className={SETTINGS_NOTE_CLASS_NAME}>{renderNote(h.agentNote)}</p>}
        </div>
      )}
    </>
  );
}

// --- compute (kubernetes) -------------------------------------------------------

function K8sHealthBadge({ s }: { s: K8sSettings }) {
  if (!s.configured) return <Badge>{m.settings_page_not_configured()}</Badge>;
  const p = s.preflight;
  if (!p.kubectlFound) return <Badge variant="error">{m.settings_page_kubectl_not_found()}</Badge>;
  if (!p.reachable) return <Badge variant="error">{m.settings_page_cluster_unreachable()}</Badge>;
  if (!p.canCreateJobs) return <Badge variant="error">{m.settings_page_no_job_create_permission()}</Badge>;
  return <Badge variant="success">{m.settings_page_connected()}</Badge>;
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_checking_kubectl()}
        </LoadingRow>
      ) : (
        <>
          <div className={COMPUTE_DETAILS_CLASS_NAME}>
            <span className="k">{m.settings_page_cluster()}</span>
            <span className="v">
              <K8sHealthBadge s={settings} />
            </span>
          </div>
          {settings.preflight.error && (
            <p className={COMPUTE_DIAGNOSTIC_CLASS_NAME}>{settings.preflight.error}</p>
          )}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <div className="row2">
              <label>
                {m.settings_page_context()}
                <OptionPicker
                  choices={[
                    {
                      id: "",
                        label: settings.currentContext ? m.settings_kubectl_default_context({ context: ltr(settings.currentContext) }) : m.settings_kubectl_default(),
                    },
                    ...(context && !settings.contexts.includes(context)
                      ? [{ id: context, label: m.settings_not_in_kubeconfig({ context: ltr(context) }) }]
                      : []),
                    ...settings.contexts.map((item) => ({ id: item, label: item })),
                  ]}
                  value={context}
                  variant="field"
                  dropDown
                  disabled={saving}
                  onSelect={setContext}
               />
              </label>
              <label>
                {m.settings_page_namespace()}
                <input
                  type="text"
                  value={namespace}
                  onChange={(e) => setNamespace(e.target.value)}
                  placeholder={m.settings_page_default()}
                  autoComplete="off"
                  spellCheck={false}
               />
              </label>
            </div>
            {error && <div className="error">{error}</div>}
            <div className="actions">
              <Button variant="primary" type="submit" disabled={saving || unchanged}>
                {saving ? m.common_saving() : m.common_save()}
              </Button>
            </div>
          </form>
          <section className="mt-7">
            <h3 className="mt-0 mx-0 mb-1.5 text-base font-semibold text-text">{m.settings_page_run_manifest()}</h3>
            <p className="m-0 font-sans text-sm leading-relaxed text-text">
              {m.settings_manifest_description({ placeholder: ltr("{{ORX_RUN}}"), command: ltr("--manifest <path>") })}
            </p>
          </section>
        </>
      )}
    </>
  );
}

// --- compute (modal) ------------------------------------------------------------

const MODAL_TOKEN_LABELS: Record<ModalTokenSource, () => string> = {
  env: m.settings_modal_token_env,
  syncedEnv: m.settings_modal_token_synced,
  modalToml: m.settings_modal_token_file,
};

function ModalBadge({ s }: { s: ModalSettings }) {
  if (s.ready) return <Badge variant="success">{m.settings_page_connected()}</Badge>;
  if (!s.tokenConfigured && !s.modalImportable) return <Badge>{m.settings_page_not_set_up()}</Badge>;
  if (!s.modalImportable)
    return <Badge variant="error">{s.envProvisioned ? m.settings_env_broken() : m.settings_env_not_built()}</Badge>;
  if (!s.tokenConfigured) return <Badge variant="error">{m.settings_page_no_token()}</Badge>;
  return <Badge>{m.settings_page_unknown()}</Badge>;
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !s ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_checking_modal()}
        </LoadingRow>
      ) : (
        <>
          <div className={COMPUTE_DETAILS_CLASS_NAME}>
            <span className="k">{m.settings_page_status()}</span>
            <span className="v">
              <ModalBadge s={s} />
            </span>
            <span className="k">{m.settings_page_environment()}</span>
            <span className="v">
              {s.modalImportable
                ? m.settings_page_ready()
                : s.envProvisioned
                  ? m.settings_modal_import_failing()
                  : m.settings_not_built_yet()}
            </span>
            <span className="k">{m.settings_page_token()}</span>
            <span className="v">
              {s.tokenSource ? MODAL_TOKEN_LABELS[s.tokenSource]() : m.settings_page_not_configured()}
            </span>
          </div>
          {!s.tokenConfigured && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              {m.settings_modal_token_help({ command: ltr("modal token new"), id: ltr("MODAL_TOKEN_ID"), secret: ltr("MODAL_TOKEN_SECRET") })}
            </p>
          )}
          {s.error && s.envProvisioned && !s.modalImportable && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>{s.error}</p>
          )}
          {error && <div className="error">{error}</div>}
          {!s.modalImportable && (
            <div className="mt-6 flex justify-end">
              <Button variant="primary" onClick={() => void provision()} disabled={provisioning}>
                {provisioning ? m.settings_setting_up_environment() : m.settings_set_up_environment()}
              </Button>
            </div>
          )}
        </>
      )}
    </>
  );
}

// --- compute (ssh) ---------------------------------------------------------------

const CONNECTION_BADGE_IDLE_CLASS = "rounded-sm border-border-strong bg-surface text-subtext";
const CONNECTION_BADGE_CONNECTING_CLASS = "rounded-sm border-accent-blue bg-accent-blue-subtle text-accent-blue";
const SSH_MASTER_POLL_MS = 5_000;

function useSshMasterStatuses(hosts: string[]) {
  const [statuses, setStatuses] = useState<Record<string, boolean>>({});
  const hostsKey = hosts.join("\0");

  useEffect(() => {
    const activeHosts = hostsKey ? hostsKey.split("\0") : [];
    if (activeHosts.length === 0) {
      setStatuses({});
      return;
    }
    let cancelled = false;
    const refresh = async () => {
      const results = await Promise.all(activeHosts.map(async (host) => {
        try {
          return [host, (await getSshMasterStatus(host)).running] as const;
        } catch {
          return null;
        }
      }));
      if (cancelled) return;
      setStatuses((current) => {
        const next: Record<string, boolean> = {};
        for (const result of results) {
          if (result) next[result[0]] = result[1];
        }
        // Preserve the last known value when only one status request fails.
        for (const host of activeHosts) {
          if (next[host] === undefined && current[host] !== undefined) next[host] = current[host];
        }
        return next;
      });
    };
    void refresh();
    const interval = window.setInterval(refresh, SSH_MASTER_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [hostsKey]);

  const markRunning = (host: string) => setStatuses((current) => ({ ...current, [host]: true }));
  return [statuses, markRunning] as const;
}

function HostTestCell({ test, connecting, masterRunning }: { test: SshPreflight | undefined; connecting: boolean; masterRunning: boolean | undefined }) {
  if (connecting)
    return (
      <span role="status">
        <Badge className={CONNECTION_BADGE_CONNECTING_CLASS}>{m.settings_connecting()}</Badge>
      </span>
    );
  if (test === undefined) return <Badge className={CONNECTION_BADGE_IDLE_CLASS}>{m.settings_page_not_checked()}</Badge>;
  const missingTools = test.missingTools ?? [];
  const disconnected = test.reachable && test.toolsFound && masterRunning === false;
  const badge = !test.reachable ? (
    <Badge className="rounded-sm" variant="error">{m.settings_page_failed()}</Badge>
  ) : !test.toolsFound ? (
    <Badge className="rounded-sm" variant="error">
      {missingTools.length === 1 ? m.settings_needs_tool({ tool: ltr(missingTools[0]) }) : m.settings_needs_tools()}
    </Badge>
  ) : disconnected ? (
    <Badge className="rounded-sm" variant="warning">{m.settings_disconnected()}</Badge>
  ) : (
    <Badge className="rounded-sm" variant="success">{m.settings_page_ready()}</Badge>
  );
  return (
    <div className="flex items-center gap-4" role="status">
      {badge}
      {!disconnected && (
        <span className="ssh-tested-at whitespace-nowrap text-xs text-subtext">{timeAgo(test.testedAt)}</span>
      )}
    </div>
  );
}

function SshSection({ remote = false }: { remote?: boolean }) {
  const [hosts, setHosts] = useState<SshHost[] | null>(null);
  const [configOpen, setConfigOpen] = useState(false);
  const [configVersion, setConfigVersion] = useState(0);
  const [tests, setTests] = useState<Record<string, SshPreflight>>({});
  const [expandedHosts, setExpandedHosts] = useState<Record<string, boolean>>({});
  const [connectingHost, setConnectingHost] = useState<string | null>(null);
  const [connectionFailed, setConnectionFailed] = useState(false);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const checkedHosts = remote ? [] : hosts
    ?.filter((host) => {
      const test = tests[host.host] ?? host.lastTest;
      return test?.reachable && test.toolsFound;
    })
    .map((host) => host.host) ?? [];
  const [masterRunning, markMasterRunning] = useSshMasterStatuses(checkedHosts);

  useEffect(() => {
    getSshHosts()
      .then(setHosts)
      .catch(() => setHosts([]));
  }, [configVersion]);

  function connect(host: string) {
    setConnectionFailed(false);
    setConnectionAttempt((attempt) => attempt + 1);
    setConnectingHost(host);
    setExpandedHosts((expanded) => ({ ...expanded, [host]: true }));
  }

  function cancelConnect() {
    setConnectionFailed(false);
    setConnectingHost(null);
  }

  function toggle(host: string, open: boolean) {
    setExpandedHosts((expanded) => ({ ...expanded, [host]: !open }));
  }

  return (
    <>
      <div className="mb-3 flex justify-end">
        <Button variant="ghost" onClick={() => setConfigOpen(true)}>
          <Settings size={14} /> {m.ssh_configure_hosts()}
        </Button>
      </div>
      {hosts === null ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_reading_ssh_config()}
        </LoadingRow>
      ) : hosts.length === 0 ? (
        <p className="settings-empty mt-1 mx-0 mb-0 text-base text-subtext">{m.settings_page_no_hosts_found_in_ssh_config()}</p>
      ) : (
        <div className="border-y border-border-variant divide-y divide-border-variant">
          {hosts.map((h) => {
            // Session-local result wins; the persisted one covers restarts.
            const hostTest = tests[h.host] ?? h.lastTest;
            const connecting = connectingHost === h.host;
            const open = expandedHosts[h.host] ?? false;
            const hasTerminal = !remote && (connecting || hostTest?.reachable === false);
            const address =
              `${h.user ? `${h.user}@` : ""}${h.hostname ?? h.host}${h.port ? `:${h.port}` : ""}`;
            return (
              <div key={h.host}>
                <div
                  className="flex items-center gap-3 py-3 px-2"
                >
                  <div className="flex min-w-0 flex-1 items-center gap-2.5">
                    {hasTerminal ? (
                      <button
                        type="button"
                        className="flex-none inline-flex items-center p-0.5 rounded-sm [&:hover]:bg-panel"
                        aria-expanded={open}
                        aria-label={open ? m.a11y_collapse_item({ name: ltr(h.host) }) : m.a11y_expand_item({ name: ltr(h.host) })}
                        onClick={(event) => {
                          event.stopPropagation();
                          toggle(h.host, open);
                        }}
                      >
                        <ChevronDown
                          size={15}
                          className={`text-muted transition-transform duration-120 ease-standard${open ? " rotate-180" : ""}`}
                       />
                      </button>
                    ) : (
                      <span className="w-5 flex-none" aria-hidden="true" />
                    )}
                    <div className="min-w-0">
                      <div className="truncate text-base font-medium text-text" title={h.host}>{h.host}</div>
                      <div className="mt-1 truncate text-sm text-subtext" title={address}>{address}</div>
                    </div>
                  </div>
                  {!remote && <div className="grid flex-none grid-cols-[8.5rem_5rem] items-center gap-x-12">
                    <div className="text-start">
                      <HostTestCell test={hostTest} connecting={connecting && !connectionFailed} masterRunning={masterRunning[h.host]} />
                    </div>
                    <Button size="small"
                      type="button"
                      className="justify-self-end"
                      onClick={(event) => {
                        event.stopPropagation();
                        if (connecting && !connectionFailed) cancelConnect();
                        else connect(h.host);
                      }}
                      disabled={!connecting && connectingHost !== null && !connectionFailed}
                    >
                      {connecting
                        ? connectionFailed
                          ? m.app_retry()
                          : m.settings_page_cancel()
                        : hostTest?.reachable === false
                          ? m.app_retry()
                          : hostTest
                          ? m.settings_reconnect()
                          : m.settings_connect()}
                    </Button>
                  </div>}
                </div>
                {hasTerminal && (open || connecting) && (
                  <div className={`border-t border-t-border-variant py-3 pe-2 ps-10${open ? "" : " hidden"}`}>
                    {!connecting && hostTest?.error && (
                      <SshTerminalTranscript host={h.host} transcript={hostTest.error} />
                    )}
                    {connecting && (
                      <SshConnectTerminal
                        key={connectionAttempt}
                        host={h.host}
                        backend="ssh"
                        active={open}
                        onComplete={(complete) => {
                          if (complete.backend !== "ssh") return;
                          setTests((tests) => ({ ...tests, [h.host]: complete.result }));
                          markMasterRunning(h.host);
                          setConnectionFailed(false);
                          setConnectingHost(null);
                        }}
                        onError={(error) => {
                          setConnectionFailed(true);
                          setTests((tests) => ({
                            ...tests,
                            [h.host]: {
                              reachable: false,
                              toolsFound: false,
                              missingTools: [],
                              error,
                              testedAt: Date.now(),
                            },
                          }));
                        }}
                      />
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
      {configOpen && (
        <SshConfigDialog
          onClose={() => setConfigOpen(false)}
          onSaved={() => setConfigVersion((version) => version + 1)}
        />
      )}
    </>
  );
}

// --- compute (slurm) --------------------------------------------------------------

/** First failing check wins, like K8sHealthBadge. */
function SlurmTestBadge({ test, connecting, masterRunning }: { test: SlurmPreflight | null; connecting: boolean; masterRunning: boolean | undefined }) {
  if (connecting) return <Badge className={CONNECTION_BADGE_CONNECTING_CLASS}>{m.settings_connecting()}</Badge>;
  if (test === null) return <Badge className={CONNECTION_BADGE_IDLE_CLASS}>{m.settings_page_not_checked()}</Badge>;
  if (!test.reachable) return <Badge className="rounded-sm" variant="error">{m.settings_page_failed()}</Badge>;
  if (!test.slurmFound) return <Badge className="rounded-sm" variant="error">{m.settings_page_no_slurm_cli()}</Badge>;
  if (!test.toolsFound) return <Badge className="rounded-sm" variant="error">{m.settings_page_missing_bash_tar()}</Badge>;
  if (masterRunning === false) return <Badge className="rounded-sm" variant="warning">{m.settings_disconnected()}</Badge>;
  return <Badge className="rounded-sm" variant="success">{m.settings_page_ready()}</Badge>;
}

function SlurmSection({ remote = false }: { remote?: boolean }) {
  const [settings, setSettings] = useState<SlurmSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [host, setHost] = useState("");
  const [partition, setPartition] = useState("");
  const [account, setAccount] = useState("");
  const [timeLimit, setTimeLimit] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<SlurmPreflight | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectionFailed, setConnectionFailed] = useState(false);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const readyHost = !remote && host && test?.reachable && test.slurmFound && test.toolsFound ? [host] : [];
  const [masterRunning, markMasterRunning] = useSshMasterStatuses(readyHost);

  function connect() {
    setConnectionFailed(false);
    setConnectionAttempt((attempt) => attempt + 1);
    setConnecting(true);
  }

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

  return (
    <>
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading_slurm_settings()}
        </LoadingRow>
      ) : (
        <>
          {!connecting && test?.error && <p className={COMPUTE_DIAGNOSTIC_CLASS_NAME}>{test.error}</p>}
          {test && test.partitions.length > 0 && (
            <div className={COMPUTE_DETAILS_CLASS_NAME}>
              <span className="k">{m.settings_page_partitions()}</span>
              <span className="v">{test.partitions.join(", ")}</span>
            </div>
          )}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <div className="row2">
              <label>
                {m.settings_page_login_node()}
                <OptionPicker
                  choices={[
                    { id: "", label: m.settings_page_not_set_pass_host_per_launch() },
                    ...(host && !settings.hosts.some((item) => item.host === host)
                      ? [{ id: host, label: `${host} (not in ~/.ssh/config)` }]
                      : []),
                    ...settings.hosts.map((item) => ({ id: item.host, label: item.host })),
                  ]}
                  value={host}
                  variant="field"
                  dropDown
                  disabled={saving || connecting}
                  onSelect={(id) => {
                    setHost(id);
                    setTest(null); // a badge earned by cluster A must not vouch for cluster B
                    setConnecting(false);
                    setConnectionFailed(false);
                  }}
               />
              </label>
              <label>
                {m.settings_page_partition()}
                <input
                  type="text"
                  list="slurm-partitions"
                  value={partition}
                  onChange={(e) => setPartition(e.target.value)}
                  placeholder={m.settings_page_cluster_default()}
                  autoComplete="off"
                  spellCheck={false}
               />
                <datalist id="slurm-partitions">
                  {test?.partitions.map((p) => <option key={p} value={p} />)}
                </datalist>
              </label>
            </div>
            <div className="row2">
              <label>
                {m.settings_page_account()}
                <input
                  type="text"
                  value={account}
                  onChange={(e) => setAccount(e.target.value)}
                  placeholder={m.settings_page_cluster_default()}
                  autoComplete="off"
                  spellCheck={false}
               />
              </label>
              <label>
                {m.settings_page_time_limit()}
                <input
                  type="text"
                  value={timeLimit}
                  onChange={(e) => setTimeLimit(e.target.value)}
                  placeholder={m.settings_page_cluster_default_e_g_4h_30m()}
                  autoComplete="off"
                  spellCheck={false}
               />
              </label>
            </div>
            {error && <div className="error">{error}</div>}
            <div className="actions">
              <Button variant="primary" type="submit" disabled={saving || unchanged || connecting}>
                {saving ? m.common_saving() : m.common_save()}
              </Button>
              {!remote && (
                <Button
                  type="button"
                  onClick={() => {
                    if (connecting && !connectionFailed) {
                      setConnectionFailed(false);
                      setConnecting(false);
                    } else {
                      connect();
                    }
                  }}
                  disabled={!host}
                  title={host ? undefined : m.settings_pick_login_node()}
                >
                  {connecting
                    ? connectionFailed
                      ? m.app_retry()
                      : m.settings_page_cancel()
                    : test
                      ? m.settings_reconnect()
                      : m.settings_connect()}
                </Button>
              )}
              <span role="status">
                <SlurmTestBadge
                  test={test}
                  connecting={connecting && !connectionFailed}
                  masterRunning={masterRunning[host]}
                />
              </span>
            </div>
          </form>
          {!remote && connecting && (
            <SshConnectTerminal
              key={connectionAttempt}
              host={host}
              backend="slurm"
              onComplete={(complete) => {
                if (complete.backend !== "slurm") return;
                setTest(complete.result);
                markMasterRunning(host);
                setConnectionFailed(false);
                setConnecting(false);
              }}
              onError={(error) => {
                setConnectionFailed(true);
                setTest({
                  reachable: false,
                  slurmFound: false,
                  toolsFound: false,
                  partitions: [],
                  error,
                });
              }}
            />
          )}
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading_ray_settings()}
        </LoadingRow>
      ) : (
        <>
          <div className={COMPUTE_DETAILS_CLASS_NAME}>
            <span className="k">{m.settings_page_effective_url()}</span>
            <span className="v">{settings.resolvedAddress}</span>
            <span className="k">{m.settings_page_source()}</span>
            <span className="v">{settings.source}</span>
            {preflight?.reachable && preflight.rayVersion && (
              <>
                <span className="k">{m.settings_page_ray_version()}</span>
                <span className="v">{preflight.rayVersion}</span>
              </>
            )}
          </div>
          {preflight?.error && <p className={COMPUTE_DIAGNOSTIC_CLASS_NAME}>{preflight.error}</p>}
          <form className={FORM_CLASS_NAME} onSubmit={submit}>
            <label>
              {m.settings_page_jobs_dashboard_url()}
              <input
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
              <Button variant="primary" type="submit" disabled={saving || unchanged}>
                {saving ? m.common_saving() : m.common_save()}
              </Button>
              <Button
                type="button"

                onClick={() => void runPreflight()}
                disabled={test === "testing"}
              >
                {m.settings_page_test_connection()}
              </Button>
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
  if (test === "testing") return <Badge>{m.settings_page_testing()}</Badge>;
  if (test.reachable) return <Badge variant="success">{m.settings_page_reachable()}</Badge>;
  return <Badge variant="error">{m.settings_page_failed()}</Badge>;
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !hw ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_detecting_hardware()}
        </LoadingRow>
      ) : (
        <div className={COMPUTE_DETAILS_CLASS_NAME}>
          <span className="k">{m.settings_page_hostname()}</span>
          <span className="v">{hw.hostname}</span>
          <span className="k">{m.settings_page_system()}</span>
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !s ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_checking_credentials()}
        </LoadingRow>
      ) : !s.loggedIn ? (
        <p className={SETTINGS_NOTE_CLASS_NAME}>
          {m.settings_login_help({ command: ltr("orx login") })}
        </p>
      ) : (
        <>
          <div className={COMPUTE_DETAILS_CLASS_NAME}>
            <span className="k">{m.settings_page_status()}</span>
            <span className="v">
              <Badge variant="success">{m.settings_page_signed_in()}</Badge>
            </span>
            <span className="k">{m.settings_page_orgs()}</span>
            <span className="v">{s.orgs.length > 0 ? s.orgs.join(", ") : "—"}</span>
            <span className="k">{m.settings_page_ssh_key()}</span>
            <span className="v">
              {s.sshKeyStatus === "matched" ? (
                <Badge variant="success">{m.settings_page_on_this_computer()}</Badge>
              ) : s.sshKeyStatus === "no_local_match" ? (
                <Badge variant="warning">{m.settings_page_not_on_this_computer()}</Badge>
              ) : s.sshKeyStatus === "none_registered" ? (
                <Badge variant="error">{m.settings_page_none_registered()}</Badge>
              ) : (
                <Badge>{m.settings_page_unknown()}</Badge>
              )}
            </span>
          </div>
          {s.sshKeyStatus === "none_registered" &&
            (s.sshKeyPath ? (
              <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>
                {m.settings_page_add_one_with()} <code>orx ssh-key add {s.sshKeyPath}</code>.
              </p>
            ) : (
              <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>
                {m.settings_page_no_key_on_this_computer_yet_create_one()}{" "}
                <code>ssh-keygen -t ed25519</code>{m.settings_page_then_add_it_with()}{" "}
                <code>orx ssh-key add</code>.
              </p>
            ))}
          {s.sshKeyStatus === "no_local_match" &&
            (s.sshKeyPath ? (
              <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>
                {m.settings_register_computer_help({ register: ltr(`orx ssh-key add ${s.sshKeyPath}`), load: ltr("ssh-add") })}
              </p>
            ) : (
              <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>
                {m.settings_page_no_key_on_this_computer_to_register_load()}{" "}
                <code>ssh-add</code>{m.settings_page_or_create_one_with()}{" "}
                <code>ssh-keygen -t ed25519</code>.
              </p>
            ))}
          {s.error && <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>{s.error}</p>}
        </>
      )}
    </>
  );
}

// --- compute -----------------------------------------------------------------

const TARGET_LABELS: Record<ComputeTargetId, () => string> = {
  local: m.compute_target_local,
  tinker: m.compute_target_tinker,
  hf: m.compute_target_hf,
  modal: m.compute_target_modal,
  k8s: m.compute_target_k8s,
  ssh: m.compute_target_ssh,
  slurm: m.compute_target_slurm,
  ray: m.compute_target_ray,
  openresearch: m.compute_target_openresearch,
};

const TARGET_CARD_DESCRIPTIONS: Record<ComputeTargetId, () => string> = {
  local: m.compute_description_local,
  ssh: m.compute_description_ssh,
  tinker: m.compute_description_tinker,
  hf: m.compute_description_hf,
  modal: m.compute_description_modal,
  k8s: m.compute_description_k8s,
  slurm: m.compute_description_slurm,
  ray: m.compute_description_ray,
  openresearch: m.compute_description_openresearch,
};

/** Kind strings from the runs table — reuses the instances-table logos. */
const TARGET_KIND: Record<ComputeTargetId, string> = {
  local: "local_job",
  tinker: "tinker_job",
  hf: "hf_job",
  modal: "modal_job",
  k8s: "k8s_job",
  ssh: "ssh_job",
  slurm: "slurm_job",
  ray: "ray_job",
  openresearch: "openresearch_job",
};

const TARGET_USAGE: Record<ComputeTargetId, () => string> = {
  local: m.compute_usage_local,
  ssh: m.compute_usage_ssh,
  tinker: m.compute_usage_tinker,
  hf: m.compute_usage_hf,
  modal: m.compute_usage_modal,
  k8s: m.compute_usage_k8s,
  slurm: m.compute_usage_slurm,
  ray: m.compute_usage_ray,
  openresearch: m.compute_usage_openresearch,
};

function targetConnection(target: ComputeTargetSummary): string {
  switch (target.id) {
    case "local":
      return m.compute_connection_local();
    case "ssh":
      return m.compute_connection_ssh({ summary: ltr(target.summary) });
    case "tinker":
      return m.compute_connection_tinker({ summary: ltr(target.summary) });
    case "hf":
      return m.compute_connection_hf({ summary: ltr(target.summary) });
    case "modal":
      return m.compute_connection_modal({ summary: ltr(target.summary) });
    case "k8s":
      return m.compute_connection_k8s({ summary: ltr(target.summary) });
    case "slurm":
      return m.compute_connection_slurm({ summary: ltr(target.summary) });
    case "ray":
      return m.compute_connection_ray({ summary: ltr(target.summary) });
    case "openresearch":
      return m.compute_connection_openresearch({ summary: ltr(target.summary) });
  }
}

function BackendOverview({ target }: { target: ComputeTargetSummary }) {
  return (
    <dl className="m-0 mt-8 grid grid-cols-[9rem_minmax(0,1fr)] gap-x-5 gap-y-4 font-sans">
      <dt className="text-sm font-medium text-subtext">{m.settings_page_how_it_connects()}</dt>
      <dd className="m-0 text-base leading-relaxed text-text">{targetConnection(target)}</dd>
      <dt className="text-sm font-medium text-subtext">{m.settings_page_what_happens()}</dt>
      <dd className="m-0 text-base leading-relaxed text-text">{TARGET_USAGE[target.id]()}</dd>
    </dl>
  );
}

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

const CUSTOM_FLAVOR_ID = "__custom__";

function isCustomFlavor(backend: ComputeTargetId, flavor: string) {
  return Boolean(flavor && !(FLAVOR_SUGGESTIONS[backend] ?? []).includes(flavor));
}

function DefaultDestinationEditor({
  settings,
  projectId,
  onSaved,
}: {
  settings: ComputeSettings;
  projectId?: string;
  onSaved: (settings: ComputeSettings) => void;
}) {
  const savedBackend = settings.configuredDefaultBackend ?? settings.defaultBackend ?? "local";
  const savedFlavor = settings.defaultFlavor ?? "";
  const [backend, setBackend] = useState(savedBackend);
  const [flavor, setFlavor] = useState(savedFlavor);
  const [customFlavor, setCustomFlavor] = useState(isCustomFlavor(savedBackend, savedFlavor));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const target = settings.targets.find((candidate) => candidate.id === backend);
  const choices = settings.targets.filter(
    (candidate) => candidate.configured || candidate.id === savedBackend,
  );
  const flavored = FLAVORED_TARGETS.includes(backend);
  const flavorRequired = FLAVOR_REQUIRED.includes(backend);
  const flavorSuggestions = FLAVOR_SUGGESTIONS[backend] ?? [];
  const unchanged =
    backend === savedBackend && (!flavored || flavor.trim() === savedFlavor);
  const destination = TARGET_LABELS[backend]();
  const helperText =
    saving
      ? m.settings_updating_default_destination()
      : flavorRequired && !flavor.trim()
        ? m.settings_choose_flavor_for_runs({ destination })
        : backend === "ssh"
          ? m.settings_new_runs_use_ssh()
          : m.settings_new_runs_use_destination({ destination });

  useEffect(() => {
    setBackend(savedBackend);
    setFlavor(savedFlavor);
    setCustomFlavor(isCustomFlavor(savedBackend, savedFlavor));
  }, [savedBackend, savedFlavor]);

  async function save(nextBackend: ComputeTargetId, nextFlavor: string) {
    const nextFlavored = FLAVORED_TARGETS.includes(nextBackend);
    if (saving || (FLAVOR_REQUIRED.includes(nextBackend) && !nextFlavor.trim())) return;
    setSaving(true);
    setError(null);
    try {
      onSaved(
        await setComputeDefault({
          backend: nextBackend,
          flavor: nextFlavored ? nextFlavor.trim() || null : null,
          projectId,
        }),
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBackend(savedBackend);
      setFlavor(savedFlavor);
      setCustomFlavor(isCustomFlavor(savedBackend, savedFlavor));
    } finally {
      setSaving(false);
    }
  }

  function changeBackend(id: string) {
    const next = settings.targets.find((candidate) => candidate.id === id);
    if (!next) return;
    setBackend(next.id);
    const nextFlavor = next.id === savedBackend ? savedFlavor : "";
    setFlavor(nextFlavor);
    setCustomFlavor(isCustomFlavor(next.id, nextFlavor));
    if (!FLAVOR_REQUIRED.includes(next.id)) void save(next.id, nextFlavor);
  }

  function changeFlavor(id: string) {
    if (id === CUSTOM_FLAVOR_ID) {
      setCustomFlavor(true);
      return;
    }
    setCustomFlavor(false);
    setFlavor(id);
    if (!flavorRequired || id) void save(backend, id);
  }

  return (
    <section className="mb-8">
      <h2 className="mt-0 mx-0 mb-2 text-lg">{m.settings_page_default_destination()}</h2>
      <div>
        <form
          className="grid grid-cols-[minmax(12rem,18rem)_minmax(12rem,18rem)] items-start gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (!unchanged) void save(backend, flavor);
          }}
        >
          <OptionPicker
            choices={choices.map((choice) => ({
              id: choice.id,
              label: TARGET_LABELS[choice.id](),
            }))}
            value={backend}
            variant="field"
            dropDown
            disabled={saving}
            renderIcon={(choice) => {
              const target = settings.targets.find((candidate) => candidate.id === choice.id);
              return target ? <BackendLogo kind={TARGET_KIND[target.id]} size={16} /> : null;
            }}
            onSelect={changeBackend}
         />
          {flavored && (
            <div>
              {customFlavor ? (
                <div className="relative">
                  <input
                    className="h-9 w-full rounded-md border border-border bg-background py-0 pe-10 ps-3 font-sans text-sm text-text outline-none focus:border-text"
                    type="text"
                    value={flavor}
                    onChange={(event) => setFlavor(event.target.value)}
                    onBlur={() => {
                      if (flavorRequired && !flavor.trim()) {
                        if (backend === savedBackend) {
                          setFlavor(savedFlavor);
                          setCustomFlavor(isCustomFlavor(savedBackend, savedFlavor));
                        }
                        return;
                      }
                      if (!unchanged) void save(backend, flavor);
                    }}
                    placeholder={m.settings_page_custom_flavor()}
                    autoFocus
                    autoComplete="off"
                    spellCheck={false}
                    disabled={saving}
                 />
                  <button
                    type="button"
                    className="absolute inset-y-0 end-0 inline-flex w-9 items-center justify-center text-muted hover:text-text"
                    aria-label={m.settings_page_choose_a_preset_flavor()}
                    title={m.settings_page_choose_a_preset_flavor()}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => setCustomFlavor(false)}
                  >
                    <ChevronDown size={12} />
                  </button>
                </div>
              ) : (
                <OptionPicker
                  choices={[
                    {
                      id: "",
                      label: flavorRequired ? m.settings_choose_flavor() : m.settings_no_default_flavor(),
                    },
                    ...(flavor && !flavorSuggestions.includes(flavor)
                      ? [{ id: flavor, label: m.settings_custom_value({ value: ltr(flavor) }) }]
                      : []),
                    ...flavorSuggestions.map((suggestion) => ({
                      id: suggestion,
                      label: suggestion,
                    })),
                    { id: CUSTOM_FLAVOR_ID, label: m.settings_page_custom_flavor_1c8a207() },
                  ]}
                  value={flavor}
                  variant="field"
                  dropDown
                  disabled={saving}
                  onSelect={changeFlavor}
               />
              )}
            </div>
          )}
        </form>
        {error && <div className="error mt-2.5">{error}</div>}
        {target && !target.configured && (
          <p className={SETTINGS_NOTE_CLASS_NAME}>
            {m.settings_page_this_saved_destination_is_not_configured_set_it()}
          </p>
        )}
      </div>
      <p className="mt-2 mb-0 text-sm leading-relaxed text-subtext">{helperText}</p>
    </section>
  );
}

function TargetTile({
  target,
  isDefault,
  onOpen,
}: {
  target: ComputeTargetSummary;
  isDefault: boolean;
  onOpen: () => void;
}) {
  const setupLabel = target.unverified
    ? m.settings_check_setup()
    : target.id === "openresearch"
      ? m.settings_sign_in()
      : target.id === "ray"
        ? m.settings_connect()
        : m.settings_set_up();

  return (
    <button
      type="button"
      className="group flex min-h-41 w-full flex-col items-start rounded-lg border border-border bg-background p-5 text-start font-sans transition-colors duration-120 ease-standard hover:border-text hover:bg-surface disabled:cursor-default disabled:opacity-52"
      onClick={onOpen}
      disabled={!target.enabled}
    >
      <span className="flex h-16 w-40 flex-none items-center justify-start">
        <BackendLogo kind={TARGET_KIND[target.id]} size={48} />
      </span>
      <span className="mt-5 text-lg font-semibold text-text">{TARGET_LABELS[target.id]()}</span>
      <span className="mt-1 line-clamp-2 min-h-9 text-sm leading-normal text-text">
        {TARGET_CARD_DESCRIPTIONS[target.id]()}
      </span>
      <span className="mt-auto flex w-full items-center justify-between gap-3 pt-3 text-sm">
        <span className={isDefault ? "font-medium text-primary" : "text-subtext"}>
          {isDefault ? m.settings_page_default_808d7dc() : target.configured ? m.settings_view_settings() : setupLabel}
        </span>
        <span className="text-subtext transition-transform duration-120 ease-standard group-hover:translate-x-0.5" aria-hidden="true">
          <ArrowRight size={16} />
        </span>
      </span>
    </button>
  );
}

function BackendDetailPage({
  target,
  isDefault,
  onBack,
  remote,
}: {
  target: ComputeTargetSummary;
  isDefault: boolean;
  onBack: () => void;
  remote: boolean;
}) {
  return (
    <>
      <button
        type="button"
        className="settings-back mb-10 inline-flex items-center gap-2 text-sm font-medium text-subtext hover:text-text"
        onClick={onBack}
      >
        <ArrowLeft size={16} /> {m.settings_page_back_to_compute()}
      </button>
      <div className="flex items-center justify-between gap-6">
        <div className={`flex min-w-0 items-center ${target.id === "tinker" ? "gap-8" : "gap-5"}`}>
          <span className="flex h-20 w-24 flex-none items-center justify-start">
            <BackendLogo kind={TARGET_KIND[target.id]} size={72} />
          </span>
          <h1 className="m-0 min-w-0">{TARGET_LABELS[target.id]()}</h1>
        </div>
        {isDefault && (
          <Badge className="flex-none border-primary bg-primary-subtle text-primary">
            {m.settings_page_default_808d7dc()}
          </Badge>
        )}
      </div>
      <BackendOverview target={target} />
      {target.id !== "tinker" && (
        <div className="mt-8 font-sans text-base text-text [&_.settings-card]:mb-0 [&_.settings-form]:mt-6 [&_.settings-form]:border-t-0 [&_.settings-form]:pt-0 [&>.settings-form:first-child]:mt-0 [&>div:first-child]:border-t-0">
          {target.id === "local" && <LocalSection />}
          {target.id === "hf" && <HfSection />}
          {target.id === "modal" && <ModalSection />}
          {target.id === "k8s" && <K8sSection />}
          {target.id === "ssh" && <SshSection remote={remote} />}
          {target.id === "slurm" && <SlurmSection remote={remote} />}
          {target.id === "ray" && <RaySection />}
          {target.id === "openresearch" && <OpenResearchSection />}
        </div>
      )}
    </>
  );
}

function ComputeTab({
  project,
  onViewHistory,
  remote,
}: {
  project: Project | null;
  onViewHistory: () => void;
  remote: boolean;
}) {
  const [settings, setSettings] = useState<ComputeSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedTarget, setSelectedTarget] = useState<ComputeTargetId | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Monotonic guard: a POST response applied via `apply` must not be
  // overwritten by a slower background GET that was already in flight.
  const seqRef = useRef(0);

  useEffect(() => {
    seqRef.current++;
    setSettings(null);
    setSelectedTarget(null);
    setLoadError(null);
    setError(null);
  }, [project?.id]);

  // Returning to the directory refreshes summaries changed on a backend page.
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
  }, [selectedTarget, project?.id]);

  const apply = (s: ComputeSettings) => {
    seqRef.current++; // supersede any in-flight background GET
    setSettings(s);
    setError(null);
  };

  // Preserve server order except for the selected default, which leads its section.
  const targets = settings ? settings.targets : null;
  const defaultBackend = settings?.configuredDefaultBackend ?? settings?.defaultBackend;
  const orderedTargets = targets
    ? [...targets].sort(
        (a, b) => Number(b.id === defaultBackend) - Number(a.id === defaultBackend),
      )
    : null;
  const configuredTargets = orderedTargets?.filter((target) => target.configured) ?? [];
  const availableTargets = orderedTargets?.filter((target) => !target.configured) ?? [];
  const renderTarget = (target: ComputeTargetSummary) => (
    <TargetTile
      key={`${project?.id ?? "none"}:${target.id}`}
      target={target}
      isDefault={defaultBackend === target.id}
      onOpen={() => setSelectedTarget(target.id)}
   />
  );
  const selected = selectedTarget
    ? settings?.targets.find((target) => target.id === selectedTarget)
    : null;

  if (selected) {
    return (
      <BackendDetailPage
        target={selected}
        isDefault={defaultBackend === selected.id}
        onBack={() => setSelectedTarget(null)}
        remote={remote}
     />
    );
  }

  return (
    <>
      <h1>{m.settings_page_compute()}</h1>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-base leading-relaxed text-text">
        {m.settings_page_connect_compute_backends_and_choose_where_new_runs()}
      </p>
      <ComputeActivity projectId={project?.id} onViewHistory={onViewHistory} />
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_checking_compute_targets()}
        </LoadingRow>
      ) : (
        <>
          {error && <div className="error">{error}</div>}
          <DefaultDestinationEditor
            settings={settings}
            projectId={project?.id}
            onSaved={apply}
         />
          <section className="mb-8">
            <h2 className="mt-0 mx-0 mb-2 text-lg">{m.settings_page_ready_to_use()}</h2>
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
              {configuredTargets.map(renderTarget)}
            </div>
          </section>
          {availableTargets.length > 0 && (
            <section className="mb-3.5">
              <h2 className="mt-0 mx-0 mb-2 text-lg">{m.settings_page_more_compute_options()}</h2>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
                {availableTargets.map(renderTarget)}
              </div>
            </section>
          )}
        </>
      )}
    </>
  );
}

// --- environment ---------------------------------------------------------------

const SOURCE_LABELS: Record<HfTokenSource, () => string> = {
  env: m.settings_hf_source_env,
  openresearchEnv: m.settings_hf_source_openresearch,
  hfCache: m.settings_hf_source_cache,
};

function HfStatusBadge({ settings }: { settings: HfSettings }) {
  if (!settings.configured) return <Badge>{m.settings_page_not_configured()}</Badge>;
  if (!settings.valid) return <Badge variant="error">{m.settings_page_invalid_token()}</Badge>;
  return <Badge variant="success">{m.settings_page_connected()}</Badge>;
}

/** Jobs-permission detail only — configured/valid state is HfStatusBadge's job. */
function HfJobsBadge({ settings }: { settings: HfSettings }) {
  if (!settings.configured || !settings.valid) return null;
  if (settings.jobsWrite === true) return <Badge variant="success">{m.settings_page_jobs_write_ok()}</Badge>;
  if (settings.jobsWrite === false)
    return <Badge variant="error">{m.settings_page_no_job_write_permission()}</Badge>;
  return <Badge>{m.settings_page_jobs_permission_unknown()}</Badge>;
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
      {loadError ? (
        <div className="error">{loadError}</div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading_status()}
        </LoadingRow>
      ) : (
        <>
          <div className={COMPUTE_DETAILS_CLASS_NAME}>
            <span className="k">{m.settings_page_status()}</span>
            <span className="v">
              <HfStatusBadge settings={settings} />
            </span>
            <span className="k">{m.settings_page_account()}</span>
            <span className="v">{settings.username ?? "—"}</span>
            <span className="k">{m.settings_page_token()}</span>
            <span className="v">{settings.maskedToken ?? "—"}</span>
            <span className="k">{m.settings_page_source()}</span>
            <span className="v">
              {settings.source ? SOURCE_LABELS[settings.source]() : m.settings_page_not_configured()}
            </span>
            <span className="k">{m.settings_page_jobs()}</span>
            <span className="v">
              <HfJobsBadge settings={settings} />
              {(!settings.configured || !settings.valid) && "—"}
            </span>
          </div>
          {settings.source === "env" && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              {m.settings_page_hf_token_is_set_in_the_environment_and()}
            </p>
          )}
          {settings.valid && settings.jobsWrite === null && (
            <p className={SETTINGS_NOTE_CLASS_NAME}>
              {m.settings_hf_token_help({ login: ltr("hf auth login"), url: ltr("huggingface.co/settings/tokens") })}
            </p>
          )}
        </>
      )}
      <form className={FORM_CLASS_NAME} onSubmit={submit}>
        <label>
          {settings?.configured ? m.settings_replace_token() : m.settings_new_token()}
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder={m.settings_page_hf()}
            autoComplete="off"
         />
        </label>
        {error && <div className="error">{error}</div>}
        <div className="actions">
          <Button variant="primary" type="submit" disabled={!token.trim() || saving}>
            {saving ? m.settings_validating() : m.common_save()}
          </Button>
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
        <p dir="auto" className={SETTINGS_NOTE_CLASS_NAME}>
          {m.settings_page_this_value_looks_like_a_hugging_face_token()}{" "}
          <code>HF_TOKEN</code>{m.settings_page_save_it_under_that_key_if_it_apos()}
        </p>
      </td>
    </tr>
  );
}

// Keys runs typically need (HF_TOKEN is also read by orx itself), always
// shown as rows alongside custom variables.
const RECOMMENDED_ENV_KEYS = ["TINKER_API_KEY", "HF_TOKEN", "WANDB_API_KEY"];

function showEnvError(name: string, err: unknown) {
  const message = err instanceof Error ? err.message : String(err);
  showAlert(message.includes(name) ? message : `${name}: ${message}`, "error");
}

/** One variable row. Set: masked value + delete. Unset: inline value input. */
function EnvRow({
  name,
  entry,
  onVars,
}: {
  name: string;
  entry: EnvVar | undefined;
  onVars: (vars: EnvVar[]) => void;
}) {
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!value.trim() || saving) return;
    setSaving(true);
    try {
      onVars(await setEnvVar(name, value.trim()));
      setValue("");
    } catch (err) {
      showEnvError(name, err);
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
      showEnvError(name, err);
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <tr>
        <td className="font-mono text-sm">{name}</td>
        <td className="text-base text-subtext">
          {entry ? (
            <>
              {entry.maskedValue}
              {entry.inProcessEnv && <Badge>{m.settings_page_overridden_by_env()}</Badge>}
            </>
          ) : (
            <Input
              variant="inline"
              className="text-base"
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
              placeholder={m.settings_page_value()}
              aria-label={m.a11y_value_for({ name: ltr(name) })}
              autoComplete="new-password"
              disabled={saving}
           />
          )}
        </td>
        <td>
          {entry ? (
            <IconButton
              className="[&:hover:not(:disabled)]:text-accent-red"
              title={m.a11y_delete_item({ name: ltr(name) })}
              aria-label={m.a11y_delete_item({ name: ltr(name) })}
              onClick={() => void remove()}
              disabled={saving}
            >
              <Trash2 size={13} />
            </IconButton>
          ) : (
            value.trim() && (
              <Button size="small" onClick={() => void save()} disabled={saving}>
                {saving ? m.common_saving() : m.common_save()}
              </Button>
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
  onDone,
}: {
  onVars: (vars: EnvVar[]) => void;
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
      showEnvError(key.trim(), err);
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
          <Input
            autoFocus
            variant="inline"
            className="font-mono text-sm"
            type="text"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="MY_API_KEY"
            aria-label={m.settings_page_new_variable_key()}
            autoComplete="off"
            spellCheck={false}
            disabled={saving}
         />
        </td>
        <td>
          <Input
            variant="inline"
            className="text-base"
            type="password"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={m.settings_page_value()}
            aria-label={m.settings_page_new_variable_value()}
            autoComplete="new-password"
            disabled={saving}
         />
        </td>
        <td>
          <Button size="small"
            onClick={() => void save()}
            disabled={saving || !key.trim() || !value.trim()}
          >
            {saving ? m.common_saving() : m.common_save()}
          </Button>
          <IconButton
            title={m.settings_page_cancel()}
            aria-label={m.settings_page_cancel_new_variable()}
            onClick={onDone}
            disabled={saving}
          >
            <X size={13} />
          </IconButton>
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

  useEffect(() => {
    getEnvVars()
      .then(setVars)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  // Recommended keys first (fixed order), then custom variables in file order.
  const customKeys =
    vars === null ? [] : vars.map((v) => v.key).filter((k) => !RECOMMENDED_ENV_KEYS.includes(k));
  const names = [...RECOMMENDED_ENV_KEYS, ...customKeys];

  return (
    <>
      <div className="mb-4.5 flex items-center justify-between gap-4">
        <p className="m-0 text-base leading-relaxed text-text">
          {m.settings_page_variables_available_to_runs_and_the_research_agent()}
        </p>
        <Button size="small" className="shrink-0"
          onClick={() => setAdding(true)}
          disabled={adding || vars === null}
        >
          <Plus size={12} /> {m.settings_page_add_variable()}
        </Button>
      </div>
      <div className={SETTINGS_CARD_CLASS_NAME}>
        {loadError ? (
          <div className="error">{loadError}</div>
        ) : vars === null ? (
          <LoadingRow>
            <Spinner /> {m.settings_page_loading()}
          </LoadingRow>
        ) : (
          <table className="env-table w-full table-fixed border-collapse text-base [&_td:first-child]:w-[32%] [&_td:first-child]:wrap-anywhere [&_.badge]:ms-2 [&_td]:h-12 [&_td]:pt-0 [&_td]:pe-2.5 [&_td]:pb-0 [&_td]:ps-0 [&_td]:align-middle [&_td]:border-b [&_td]:border-b-border-variant [&_td:last-child]:w-29 [&_td:last-child]:whitespace-nowrap [&_td:last-child]:text-end [&_td[colspan]]:whitespace-normal [&_td[colspan]]:text-start [&_.icon-btn]:ms-2 [&_.icon-btn]:align-middle">
            <tbody>
              {names.map((name) => (
                <EnvRow
                  key={name}
                  name={name}
                  entry={vars.find((v) => v.key === name)}
                  onVars={setVars}
               />
              ))}
              {adding && (
                <AddVarRow onVars={setVars} onDone={() => setAdding(false)} />
              )}
            </tbody>
          </table>
        )}
      </div>
    </>
  );
}

// --- appearance ----------------------------------------------------------------

const THEME_OPTIONS: {
  value: ThemePreference;
  label: () => string;
  icon: typeof Monitor;
}[] = [
  { value: "system", label: m.settings_theme_system, icon: Monitor },
  { value: "light", label: m.settings_theme_light, icon: Sun },
  { value: "dark", label: m.settings_theme_dark, icon: Moon },
];

const LOCALE_CHOICES: { id: Locale; label: string }[] = [
  { id: "en", label: "English" },
  { id: "zh-CN", label: "简体中文" },
  { id: "fa", label: "فارسی" },
];

function AppearanceTab() {
  const locale = useLocale();
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
      <h2>{m.settings_appearance_heading()}</h2>
      <div className={`${SETTINGS_CARD_CLASS_NAME} mt-3`}>
        <div className={`${PROJECT_DEFAULT_ROW_CLASS_NAME} pb-3.5`}>
          <div className="project-default-title text-base font-medium">{m.settings_theme_heading()}</div>
          <div
            className="theme-segmented inline-flex flex-none gap-0.5 p-0.5 border border-border rounded-md bg-surface"
            role="radiogroup"
            aria-label={m.settings_theme_heading()}
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
                {label()}
              </button>
            ))}
          </div>
        </div>
        <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
          <div className="project-default-title text-base font-medium">{m.settings_language_heading()}</div>
          <div className="w-52 flex-none">
            <OptionPicker
              choices={LOCALE_CHOICES}
              value={locale}
              variant="field"
              dropDown
              onSelect={(next) => {
                if (isLocale(next)) setLocale(next);
              }}
           />
          </div>
        </div>
      </div>
    </>
  );
}

// --- updates -----------------------------------------------------------------

const CHANNEL_LABELS: Record<InstallChannel, () => string> = {
  installer: m.updates_channel_installer,
  "app-bundle": m.updates_channel_app,
  cargo: m.updates_channel_cargo,
  homebrew: m.updates_channel_homebrew,
  nix: m.updates_channel_nix,
  unknown: m.updates_channel_unknown,
};

/** What to do about updates when orx can't do them itself. */
const MANUAL_UPDATE_HINT: Partial<Record<InstallChannel, () => string>> = {
  cargo: m.updates_hint_cargo,
  homebrew: m.updates_hint_homebrew,
  nix: m.updates_hint_nix,
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
        <h2>{m.settings_page_updates()}</h2>
        {loadError ? (
          <div className={SETTINGS_CARD_CLASS_NAME}>
            <div className="error">{loadError}</div>
          </div>
        ) : (
          <LoadingRow>
            <Spinner /> {m.settings_page_loading()}
          </LoadingRow>
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
      <h2>{m.settings_page_updates()}</h2>
      <div className={`${SETTINGS_CARD_CLASS_NAME} mt-3`}>
        <div className={`${KV_CLASS_NAME} pb-3.5`}>
          <div className="k">{m.settings_page_version()}</div>
          <div className="v">{status.current}</div>
          <div className="k">{m.settings_page_latest()}</div>
          <div className="v">{status.latest ?? "—"}</div>
          <div className="k">{m.settings_page_install()}</div>
          <div className="v">{CHANNEL_LABELS[status.channel]()}</div>
        </div>

        {status.restartRequired && (
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-base font-medium">
                {m.settings_page_restart_to_finish_updating()}
              </div>
              <p>
                {m.settings_restart_version({ installed: ltr(status.installedVersion ?? "—"), current: ltr(status.current ?? status.installedVersion ?? "—") })}
              </p>
            </div>
          </div>
        )}

        {status.selfUpdates ? (
          <>
            <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
              <div>
                <div className="project-default-title text-base font-medium">
                  {m.settings_page_install_updates_automatically()}
                </div>
                <p>
                  {m.settings_page_new_releases_are_downloaded_and_installed_in_the()}
                  {status.envDisabled &&
                    m.settings_updates_disabled_by_env()}
                </p>
              </div>
              <Switch
                type="button"
                checked={status.autoUpdate}
                aria-label={m.settings_page_install_updates_automatically()}
                disabled={busy !== null}
                onClick={() =>
                  void run("auto", () => setAutoUpdateApi(!status.autoUpdate).then(apply))
                }
              />
            </div>
            <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
              <div>
                <div className="project-default-title text-base font-medium">
                  {status.updateAvailable ? m.settings_update_to_version({ version: ltr(status.latest ?? "—") }) : m.settings_check_for_updates()}
                </div>
                <p>
                  {status.updateAvailable
                    ? m.settings_install_release_now()
                    : m.settings_checks_automatically()}
                </p>
              </div>
              <Button size="small"
                type="button"

                disabled={busy !== null}
                onClick={() => void run("apply", () => applyUpdate().then(apply))}
              >
                {busy === "apply"
                  ? m.chat_working()
                  : status.updateAvailable
                    ? m.settings_update_now()
                    : m.settings_check_now()}
              </Button>
            </div>
          </>
        ) : (
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-base font-medium">
                {m.settings_page_orx_can_t_update_this_install()}
              </div>
              <p>
                {MANUAL_UPDATE_HINT[status.channel]?.() ??
                  m.settings_reinstall_for_updates()}
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

function TelemetryTab() {
  const [settings, setSettings] = useState<TelemetrySettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getTelemetry()
      .then(setSettings)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  const toggle = () => {
    if (!settings || saving) return;
    setSaving(true);
    setError(null);
    void setTelemetry(!settings.preferenceEnabled)
      .then(setSettings)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setSaving(false));
  };

  return (
    <>
      <h2>{m.settings_page_usage_analytics()}</h2>
      {!settings ? (
        error ? <div className="error">{error}</div> : <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
      ) : (
        <div className={`${SETTINGS_CARD_CLASS_NAME} mt-3`}>
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title inline-flex items-center gap-1.5 text-base font-medium">
                {m.settings_page_anonymous_usage_analytics()}
                {settings.locked && settings.reason && (
                  <Tooltip
                    content={`${m.settings_page_currently_off()} ${settings.reason}.`}
                    className="text-subtext"
                  >
                    <Info size={15} />
                  </Tooltip>
                )}
              </div>
              <p>{m.settings_page_no_code_prompts_file_contents_or_account_identifiers()}</p>
            </div>
            <Switch
              type="button"
              checked={settings.enabled}
              aria-label={m.settings_page_anonymous_usage_analytics()}
              disabled={saving || settings.locked}
              onClick={toggle}
            />
          </div>
          {error && <div className="error">{error}</div>}
        </div>
      )}
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
        <div className="project-default-title text-base font-medium">
          {m.settings_install_cli_title({ command: ltr("orx") })}
        </div>
        {result ? (
          <p>
            {result.alreadyCurrent
              ? m.settings_cli_already_linked({ link: ltr(result.link) })
              : m.settings_cli_linked({ link: ltr(result.link) })}
            {!result.onPath && m.settings_add_to_path({ directory: ltr(result.dir) })}
          </p>
        ) : (
          <p>
            {m.settings_install_cli_description({ command: ltr("orx") })}
          </p>
        )}
      </div>
      <Button size="small"
        type="button"

        disabled={busy !== null}
        onClick={() => install(needsForce)}
      >
        {busy === "cli" ? m.chat_working() : needsForce ? m.settings_replace_anyway() : result ? m.settings_relink() : m.settings_install()}
      </Button>
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
    return getProjectDefaults()
      .then(setSettings)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };
  useEffect(() => void load(), []);

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
      <h2>{m.settings_page_general()}</h2>
      {!settings ? (
        error ? <div className="error">{error}</div> : <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
      ) : (
        <div className={`${SETTINGS_CARD_CLASS_NAME} mt-3 project-defaults-card [&_.settings-card-head]:justify-between [&_.settings-card-head]:mb-0 [&_.settings-card-head]:pb-3 [&_.settings-card-head_h3]:m-0`}>
          <div className="settings-card-head flex items-center gap-2.5 mb-3">
            <h3>{m.settings_page_git_hub_publishing()}</h3>
            <Badge variant={settings.githubAuthenticated ? "success" : settings.ghInstalled ? "warning" : "error"}>
              {settings.githubAuthenticated ? m.settings_connected_via_github_cli() : m.settings_not_connected()}
            </Badge>
          </div>
          <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
            <div>
              <div className="project-default-title text-base font-medium">{m.settings_page_enable_git_hub_syncing_for_new_projects()}</div>
              <p>
                {m.settings_page_when_enabled_each_new_project_gets_a_private()}
              </p>
            </div>
            <Switch
              type="button"
              checked={settings.githubForNewProjects}
              aria-label={m.settings_page_enable_git_hub_syncing_for_new_projects()}
              disabled={saving || (!settings.githubAuthenticated && !settings.githubForNewProjects)}
              onClick={toggle}
            />
          </div>
          {!settings.githubAuthenticated && (
            <div className="mt-3.5 pt-3.5 border-t border-t-border-variant">
              <GitHubCliHelp ghInstalled={settings.ghInstalled} onCheck={load} />
            </div>
          )}
          {error && <div className="error">{error}</div>}
        </div>
      )}
    </>
  );
}

function GitHubCliHelp({
  ghInstalled,
  onCheck,
}: {
  ghInstalled: boolean;
  onCheck: () => Promise<void>;
}) {
  const [checking, setChecking] = useState(false);
  const check = () => {
    setChecking(true);
    void onCheck().finally(() => setChecking(false));
  };

  return (
    <>
      <p className="git-card-helper m-0 text-sm leading-relaxed text-text">
        {renderNote(ghInstalled ? m.settings_run_gh_auth_login() : m.settings_install_gh_then_login())}
      </p>
      <div className="flex flex-wrap gap-2 mt-2.5">
        {!ghInstalled && (
          <ButtonLink variant="primary"
            href="https://cli.github.com/"
            target="_blank"
            rel="noreferrer"
          >
            {m.settings_page_install_git_hub_cli()} <ExternalLink size={12} />
          </ButtonLink>
        )}
        <Button type="button" variant={ghInstalled ? "warning" : "default"} disabled={checking} onClick={check}>
          {checking ? m.common_checking() : m.settings_check_again()}
        </Button>
      </div>
    </>
  );
}

/** The Overleaf Git authentication token is machine-wide. Which Overleaf
 * *project* a paper pushes to is per-paper, and lives on the .tex tab. */
function OverleafCard() {
  const [hasToken, setHasToken] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getOverleafSettings()
      .then((s) => setHasToken(s.hasToken))
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  return (
    <div className={GIT_SETTINGS_CARD_CLASS_NAME}>
      <h3>{m.settings_page_overleaf()}</h3>
      <div className={KV_CLASS_NAME}>
        <span className="k">{m.settings_page_git_token()}</span>
        <span className="v">
          <Badge variant={hasToken ? "success" : "default"}>
            {hasToken === null ? (error ? m.model_picker_unavailable() : m.common_checking()) : hasToken ? m.settings_saved() : m.settings_not_set()}
          </Badge>
        </span>
      </div>
      <p className="git-card-helper mt-3.5 mx-0 mb-0 text-sm leading-relaxed text-text">
        {m.settings_page_with_a_token_saved_a_paper_opened_in()}
      </p>
      {hasToken ? (
        <div className={GIT_CARD_ACTIONS_CLASS_NAME}>
          <Button
            disabled={saving}
            onClick={() => {
              setSaving(true);
              setError(null);
              void deleteOverleafToken()
                .then((s) => setHasToken(s.hasToken))
                .catch((err) => setError(err instanceof Error ? err.message : String(err)))
                .finally(() => setSaving(false));
            }}
          >
            {saving ? m.settings_removing() : m.settings_remove_token()}
          </Button>
        </div>
      ) : (
        <TokenForm
          save={saveOverleafToken}
          onSaved={(result) => setHasToken(result.hasToken)}
          placeholder={m.settings_page_overleaf_git_authentication_token()}
          createHref="https://www.overleaf.com/user/settings"
       />
      )}
      {error && <div className="error">{error}</div>}
    </div>
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

  const load = (clear = true) => {
    const request = ++seqRef.current;
    if (clear) setStatus(null);
    setError(null);
    if (!project) return Promise.resolve();
    return getProjectGitStatus(project.id)
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
  useEffect(() => void load(), [project?.id]);

  const syncErrorMessage = (err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    if (message.toLowerCase().includes("archived")) {
      return m.settings_github_archived_error();
    }
    if (message.includes("(fetch first)") || message.includes("non-fast-forward")) {
      return m.settings_github_fetch_first_error();
    }
    if (message.includes("403") || message.toLowerCase().includes("permission denied")) {
      return m.settings_github_permission_error();
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
      <h1>{m.settings_page_repository()}</h1>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-base leading-relaxed text-text">
        {m.settings_repository_description({ project: project?.name ?? m.settings_current_project() })}
      </p>
      {!project ? (
        <div className={SETTINGS_CARD_CLASS_NAME}><p className={SETTINGS_NOTE_CLASS_NAME}>{m.settings_page_open_a_project_to_inspect_its_repository_and()}</p></div>
      ) : error && !status ? (
        <div className="error">{error}</div>
      ) : !status ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading()}
        </LoadingRow>
      ) : (
        <>
          <div className={GIT_SETTINGS_CARD_CLASS_NAME}>
            <h3>{m.settings_page_local_repository()}</h3>
            <div className={KV_CLASS_NAME}>
              <span className="k">{m.settings_page_path()}</span><span className="v">{status.path}</span>
              <span className="k">Git</span><span className="v">{status.gitVersion ?? m.onboarding_not_found()}</span>
              <span className="k">{m.settings_page_state()}</span><span className="v">{status.initialized ? m.settings_git_state({ branch: ltr(status.currentBranch ?? m.settings_detached()), state: status.clean ? m.settings_clean() : m.settings_has_changes() }) : m.settings_not_initialized()}</span>
              <span className="k">{m.settings_page_baseline()}</span><span className="v">{status.baselineBranch}</span>
              <span className="k">{m.settings_page_remotes()}</span><span className="v">{status.remotes.length ? status.remotes.map((remote) => `${remote.name}: ${remote.url}`).join(" · ") : m.settings_none()}</span>
            </div>
            {!status.initialized && <div className={GIT_CARD_ACTIONS_CLASS_NAME}><Button variant="primary" onClick={() => void initializeProjectGit(project.id).then(setStatus).catch((err) => setError(String(err)))}>{m.settings_page_initialize_git()}</Button></div>}
          </div>
          <div className={GIT_SETTINGS_CARD_CLASS_NAME}>
            <h3>GitHub</h3>
            <div className={KV_CLASS_NAME}>
              <span className="k">{m.settings_page_authentication()}</span><span className="v"><Badge variant={status.github.authenticated ? "success" : status.github.ghInstalled ? "warning" : "error"}>{status.github.authenticated ? m.settings_connected_via_github_cli() : m.settings_not_connected()}</Badge></span>
              <span className="k">{m.settings_page_project()}</span><span className="v">{hasGithubRepository ? <><span>{status.github.owner}/{status.github.repo}</span>{!status.github.enabled && <Badge>{m.settings_page_syncing_off()}</Badge>}</> : <Badge>{m.settings_page_local_only()}</Badge>}</span>
              {status.github.enabled && <><span className="k">{m.settings_page_sync()}</span><span className="v">{status.github.syncStatus}</span></>}
            </div>
            {!status.github.authenticated && (
              <div className="mt-3.5 pt-3.5 border-t border-t-border-variant">
                <GitHubCliHelp ghInstalled={status.github.ghInstalled} onCheck={() => load(false)} />
              </div>
            )}
            {status.github.authenticated && !status.github.enabled && (
              <>
                <p className="git-card-helper mt-3.5 mx-0 mb-0 text-sm leading-relaxed text-text">
                  {hasGithubRepository
                    ? m.settings_use_existing_github_repository()
                    : m.settings_create_private_github_repository()}
                </p>
                <div className={GIT_CARD_ACTIONS_CLASS_NAME}>
                  {hasGithubRepository && status.github.url && <ButtonLink href={status.github.url} target="_blank" rel="noreferrer">{m.settings_page_open_on_git_hub()} <ExternalLink size={12} /></ButtonLink>}
                  <Button variant="primary" disabled={saving} onClick={enableSync}>{saving ? m.repository_enabling() : m.repository_enable_syncing()}</Button>
                </div>
              </>
            )}
            {status.github.enabled && (
              <>
                <p className="git-card-helper mt-3.5 mx-0 mb-0 text-sm leading-relaxed text-text">
                  {m.settings_page_disabling_syncing_stops_automatic_pushes_compute_continues_to()}
                </p>
                <div className={GIT_CARD_ACTIONS_CLASS_NAME}>
                  {status.github.url && <ButtonLink href={status.github.url} target="_blank" rel="noreferrer">{m.settings_page_open_on_git_hub()} <ExternalLink size={12} /></ButtonLink>}
                  <Button disabled={saving} onClick={() => { setSaving(true); void disableProjectGithub(project.id).then((result) => { setStatus(result.git); onProjectUpdate(result.project); }).catch((err) => setError(err instanceof Error ? err.message : String(err))).finally(() => setSaving(false)); }}>{saving ? m.repository_updating() : m.repository_disable_syncing()}</Button>
                </div>
              </>
            )}
          </div>
          <OverleafCard />
          {publicationError && <div className="error">{syncErrorMessage(publicationError)}</div>}
          {error && <div className="error">{syncErrorMessage(error)}</div>}
        </>
      )}
      {defaultPromptOpen && (
        <div className="modal-backdrop fixed inset-0 bg-modal-backdrop-light flex items-start justify-center pt-[var(--modal-top)] px-4 pb-6 overflow-y-auto z-100" onClick={() => finishDefaultPrompt(false)}>
          <div
            className="modal max-w-[94vw] max-h-[calc(100vh_-_var(--modal-top)_-_48px)] overflow-y-auto bg-background border border-border rounded-xl shadow-modal p-6 [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-xl github-default-modal w-110 [&_>_p]:m-0 [&_>_p]:text-sm [&_>_p]:leading-relaxed [&_>_p]:text-text [&_>_.error]:mt-3.5"
            role="dialog"
            aria-modal="true"
            aria-labelledby="github-default-title"
            onClick={(event) => event.stopPropagation()}
          >
            <h2 id="github-default-title">{m.settings_page_make_git_hub_syncing_the_default()}</h2>
            <p>
              {m.settings_page_this_is_useful_when_collaborators_follow_project_changes()}
            </p>
            {defaultPromptError && <div className="error">{defaultPromptError}</div>}
            <div className="github-default-actions flex justify-end gap-2.5 mt-5.5">
              <Button disabled={defaultPromptSaving} onClick={() => finishDefaultPrompt(false)}>
                {m.settings_page_not_now()}
              </Button>
              <Button variant="primary" disabled={defaultPromptSaving} onClick={() => finishDefaultPrompt(true)}>
                {defaultPromptSaving ? m.common_saving() : m.settings_make_default()}
              </Button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

// --- storage (data directory) ------------------------------------------------

const DATA_DIR_SOURCE_LABEL: Record<DataDirSettings["source"], () => string> = {
  env: m.storage_source_env,
  config: m.storage_source_saved,
  xdg: m.storage_source_xdg,
  default: m.storage_source_default,
};

const MOVE_PHASE_LABELS: Record<string, () => string> = {
  preparing: m.storage_preparing,
  copying: m.storage_copying,
  verifying: m.storage_verifying,
  finalizing: m.storage_finalizing,
};
const movePhaseLabel = (phase: string) => MOVE_PHASE_LABELS[phase]?.() ?? phase;

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
      !window.confirm(m.storage_move_confirm({ path: ltr(trimmed) }))
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
      <h2>{m.settings_page_storage()}</h2>
      <p className="settings-sub mt-0 mx-0 mb-4.5 text-sm leading-relaxed text-subtext">
        {m.settings_storage_description()}
      </p>
      {loadError ? (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="error">{loadError}</div>
        </div>
      ) : !settings ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading()}
        </LoadingRow>
      ) : (
        <div className={SETTINGS_CARD_CLASS_NAME}>
          <div className="settings-card-head mb-3">
            <h3>{m.settings_page_data_directory()}</h3>
          </div>
          <div className={KV_CLASS_NAME}>
            <span className="k">{m.settings_page_current()}</span>
            <span className="v">{settings.current}</span>
            <span className="k">{m.settings_page_source()}</span>
            <span className="v">{DATA_DIR_SOURCE_LABEL[settings.source]()}</span>
          </div>

          {!envForced && (
            <form className={FORM_CLASS_NAME} onSubmit={startMove}>
              <label>
                {m.settings_page_new_location()}
                <input
                  className="text-sm"
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
                  {m.settings_page_ready_to_move()} {fmtBytes(validation.treeBytes ?? 0)}
                  {validation.freeBytes != null && ` — ${m.storage_free_at_target({ size: ltr(fmtBytes(validation.freeBytes)) })}`}
                  {validation.sameFilesystem ? m.storage_same_disk() : ""}.
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
                  label={movePhaseLabel(move.phase)}
                  caption={
                    move.total > 0 ? (
                      <span className="text-sm">
                        {fmtBytes(move.copied)} / {fmtBytes(move.total)}
                      </span>
                    ) : undefined
                  }
               />
              )}
              {move.kind === "done" && (
                <p className={SETTINGS_NOTE_CLASS_NAME}>
                  {m.settings_page_moved_orx_is_now_using_the_new_location()}
                  {move.oldPathLeft && (
                    <>
                      {" "}
                      {m.settings_old_copy_left({ path: ltr(move.oldPathLeft) })}
                    </>
                  )}
                </p>
              )}
              {move.kind === "error" && <div className="error">{m.settings_page_move_failed()} {move.message}</div>}

              <div className="actions">
                <Button
                  type="button"

                  onClick={check}
                  disabled={checking || !trimmed || unchanged || move.kind === "moving"}
                >
                  {checking ? m.common_checking() : m.settings_check()}
                </Button>
                <Button variant="primary"
                  type="submit"

                  disabled={!trimmed || unchanged || move.kind === "moving"}
                >
                  {move.kind === "moving" ? m.storage_moving() : m.storage_move_here()}
                </Button>
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
function runtimeLabel(inst: Run): string {
  if (isLive(inst.status)) return fmtDuration(Date.now() - inst.createdAt);
  if (inst.endedAt) return fmtDuration(inst.endedAt - inst.createdAt);
  return "—";
}

/** One section's table: backend (logo + flavor), status, started, runtime. */
function InstancesTable({ instances, emptyLabel }: { instances: Run[]; emptyLabel: string }) {
  if (instances.length === 0) {
    return <p className="instances-empty m-0 rounded-lg border border-border bg-background py-3.5 px-4 text-base text-subtext">{emptyLabel}</p>;
  }
  return (
    <div className="instances-table-wrap overflow-x-auto">
      <table className="runs-table w-full border-collapse bg-background text-base [&_th]:text-start [&_th]:text-text [&_th]:text-sm [&_th]:font-medium [&_th]:py-2 [&_th]:px-3 [&_th]:border-b [&_th]:border-b-border [&_th]:sticky [&_th]:top-0 [&_th]:bg-background [&_th]:z-1 [&_td]:py-2 [&_td]:px-3 [&_td]:border-b [&_td]:border-b-divider-faint [&_td]:whitespace-nowrap [&_tr:last-child_td]:border-b-0 [&_tr.clickable]:cursor-pointer [&_tr.clickable:hover_td]:bg-canvas">
        <thead>
          <tr>
            <th>{m.settings_page_backend()}</th>
            <th>{m.settings_page_status()}</th>
            <th>{m.settings_page_started()}</th>
            <th>{m.settings_page_runtime()}</th>
          </tr>
        </thead>
        <tbody>
          {instances.map((inst) => {
            // HF jobs carry their dashboard URL; Modal stores only a sandbox id.
            const url = typeof inst.backend?.url === "string" ? inst.backend.url : undefined;
            return (
              <tr key={inst.id}>
                <td>
                  <span className="backend-cell inline-flex items-center gap-0.5">
                    <BackendBadge backend={inst.backend} />
                    {url && (
                      <IconButtonLink size="small"
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        title={m.settings_page_open_job_page()}
                        aria-label={m.settings_page_open_job_page()}
                        onClick={(e) => e.stopPropagation()}
                      >
                        <ExternalLink size={12} />
                      </IconButtonLink>
                    )}
                  </span>
                </td>
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

function ComputeActivity({ projectId, onViewHistory }: { projectId?: string; onViewHistory: () => void }) {
  const [instances, setInstances] = useState<Run[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Re-render every 30s so live rows' Runtime keeps counting (client-side
  // only — the minute-level display doesn't warrant a refetch).
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, []);

  // Point-in-time snapshot: the page refetches on every open and Refresh updates it in place.
  const load = () => {
    if (!projectId) {
      setInstances([]);
      return;
    }
    setRefreshing(true);
    listRuns(projectId)
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
  useEffect(() => load(), [projectId]);

  const byRecent = (a: Run, b: Run) => b.createdAt - a.createdAt;
  const running = instances?.filter((i) => isLive(i.status)).sort(byRecent);
  const past = instances?.filter((i) => !isLive(i.status)).sort(byRecent);

  return (
    <section className="compute-activity [&_.count-badge]:inline-flex [&_.count-badge]:items-center [&_.count-badge]:justify-center [&_.count-badge]:min-w-4.5 [&_.count-badge]:h-4.5 [&_.count-badge]:py-0 [&_.count-badge]:px-[5px] [&_.count-badge]:rounded-md [&_.count-badge]:bg-canvas [&_.count-badge]:border [&_.count-badge]:border-border [&_.count-badge]:text-xs [&_.count-badge]:font-medium [&_.count-badge]:text-text mt-5.5 mx-0 mb-8">
      <div className="compute-activity-head flex items-start justify-between gap-5 mb-3.5 [&_h2]:flex [&_h2]:items-center [&_h2]:gap-2 [&_h2]:m-0 [&_h2]:text-lg [@media((max-width:_640px))]:items-stretch [@media((max-width:_640px))]:flex-col">
        <div>
          <h2>
            {m.settings_page_running_instances()}
            {running && running.length > 0 && <span className="count-badge">{running.length}</span>}
          </h2>
        </div>
        <div className="compute-activity-actions flex gap-2 flex-none [@media((max-width:_640px))]:justify-start">
          <Button size="small" onClick={load} disabled={refreshing}>
            <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
          </Button>
          <Button size="small" onClick={onViewHistory}>
            {past?.length ? m.instances_view_history_count({ count: fmtNumber(past.length) }) : m.instances_view_history()}
          </Button>
        </div>
      </div>
      {error && <div className="error">{error}</div>}
      {!running || !past ? (
        <LoadingRow>
          <Spinner /> {m.settings_page_loading()}
        </LoadingRow>
      ) : <InstancesTable instances={running} emptyLabel={projectId ? m.instances_nothing_running() : m.instances_select_project_runs()} />}
    </section>
  );
}

function InstanceHistory({ projectId, onBack }: { projectId?: string; onBack: () => void }) {
  const [instances, setInstances] = useState<Run[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [, setTick] = useState(0);

  useEffect(() => {
    const timer = setInterval(() => setTick((tick) => tick + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const load = () => {
    if (!projectId) {
      setInstances([]);
      return;
    }
    setRefreshing(true);
    listRuns(projectId)
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
  useEffect(load, [projectId]);

  return (
    <>
      <button type="button" className="settings-back inline-flex items-center gap-1.5 mt-0 mx-0 mb-4.5 text-subtext text-sm font-medium [&:hover]:text-text" onClick={onBack}>
        <ArrowLeft size={14} /> {m.settings_page_back_to_compute()}
      </button>
      <div className="settings-head-row flex items-center justify-between gap-2.5 [&_h1]:m-0">
        <h1>{m.settings_page_instance_history()}</h1>
        <Button size="small" onClick={load} disabled={refreshing}>
          <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
        </Button>
      </div>
      {error && <div className="error">{error}</div>}
      {!instances ? (
        <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
      ) : (
        <InstancesTable instances={instances} emptyLabel={projectId ? m.instances_none_yet() : m.instances_select_project_history()} />
      )}
    </>
  );
}

// --- embedded view -----------------------------------------------------------

type SettingsNavItem = {
  id: Tab;
  label: () => string;
  icon: React.ReactNode;
  activeTabs: Tab[];
};

const SETTINGS_SECTIONS: Tab[] = ["projects", "harnesses", "storage"];

/** Primary rail entries. Configuration sections share the Settings entry. */
export const SETTINGS_NAV: SettingsNavItem[] = [
  {
    id: "compute",
    label: m.settings_page_compute,
    icon: <Cpu size={15} />,
    activeTabs: ["compute", "instances"],
  },
  { id: "environment", label: m.settings_page_environment, icon: <SquareTerminal size={15} />, activeTabs: ["environment"] },
  {
    id: "settings",
    label: m.settings_page_settings,
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
  remote = false,
}: {
  tab: Tab;
  project: Project | null;
  githubPublicationError: string | null;
  onProjectUpdate: (project: Project) => void;
  onSelectTab: (tab: Tab) => void;
  remote?: boolean;
}) {
  const showsSettings = tab === "settings" || isSettingsSection(tab);

  return (
    <div className="settings-view max-w-readable my-0 mx-auto pt-6 px-8 pb-15 [&_h1]:mt-0 [&_h1]:mx-0 [&_h1]:mb-1.5 [&_h1]:text-3xl [&_>_.error]:text-accent-red [&_>_.error]:text-base [&_>_.error]:whitespace-pre-wrap [&_>_.error]:mt-0 [&_>_.error]:mx-0 [&_>_.error]:mb-3">
      {showsSettings && (
        <>
          <h1>{m.settings_page_settings()}</h1>
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
            {!remote && (
              <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
                <StorageTab />
              </section>
            )}
            <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
              <TelemetryTab />
            </section>
            {!remote && (
              <section className={SETTINGS_STACK_SECTION_CLASS_NAME}>
                <UpdatesTab />
              </section>
            )}
          </div>
        </>
      )}
      {tab === "compute" && (
        <ComputeTab
          project={project}
          onViewHistory={() => onSelectTab("instances")}
          remote={remote}
       />
      )}
      {tab === "instances" && (
        <InstanceHistory projectId={project?.id} onBack={() => onSelectTab("compute")} />
      )}
      {tab === "environment" && (
        <>
          <h1>{m.settings_page_environment()}</h1>
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
