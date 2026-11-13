import { useContext, useEffect, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { useMachines } from "../machineSelection";
import { useSignIn } from "./AccountChip";
import { OpenResearchAccount } from "./OpenResearchAccount";
import { LitSourceLogo } from "./LitSourceLogo";

import { useQuery } from "@tanstack/react-query";
import { useMutate } from "@clo/react-mutation";
import { Terminal as Xterm } from "@xterm/xterm";

import { m } from "../paraglide/messages.js";
import { mutCompleteOnboarding, mutInstallGit, mutUpdateUiState } from "../queries/settings";
import { ltr } from "../i18n";
import { AlertCircle, ArrowLeft, ArrowRight, RefreshCw } from "lucide-react";
import { Wordmark } from "./Wordmark";

import {
  reasoningFor,
  type AgentSelection,
  type Harness,
  type OnboardingStep,
  type Project,
  type UiState,
  AlphaXivNotConnectedError,
  type OpenResearchAccount as OpenResearchAccountInfo,
} from "../api";
import type { AddedAgent } from "./AddAgentDialog";
import { agentFor, agentOffers, pickerAgents, type PickerAgent } from "./pickerAgents";
import { AgentList } from "./SettingsPage";
import { defaultOffer, groupModelLines } from "../modelLines";
import { selectionHarness } from "../modelSelection";

import { Hyperspace } from "./Hyperspace";
import { Button, Spinner, StatusIndicator } from "./ui";

const ONB_CARD_META_CLASS_NAME = [
  "onb-card-meta text-sm text-subtext [&_code]:font-mono",
  "[&_code]:text-xs [&_code]:bg-surface",
  "[&_code]:border [&_code]:border-border-variant [&_code]:rounded-xs",
  "[&_code]:py-px [&_code]:px-1 [&_code]:whitespace-nowrap",
].join(" ");

const GIT_RETRY_HINT_CLASS_NAME = [
  "onb-gate-hint mt-4.5 mx-0 mb-0 text-base font-medium leading-normal",
  "text-text onb-git-hint mt-2",
].join(" ");

const FINISH_ERROR_CLASS_NAME = [
  "onb-gate-hint mt-4.5 mx-0 mb-0 text-base font-medium leading-normal",
  "text-text",
].join(" ");

/** In order — the API funnel keys on these names for all time, so renumbering
 * the screens must not redefine a historical step. */
const ONBOARDING_STEP_NAMES: readonly OnboardingStep[] = ["welcome", "environment", "profile"];

type OnboardingProps = { preferredAgent: AgentSelection | null } & (
  | { remote: false; onDone: (project: Project, selection: AgentSelection) => void }
  | { remote: true; host: string; onDone: (state: UiState) => void }
);

export function Onboarding(props: OnboardingProps) {
  const { remote, preferredAgent } = props;
  const { id: elementId } = useContext(WorkspaceDomContext);

  const workspace = useWorkspace();
  const { socketUrl, queryClient } = workspace;
  const { captureUiEvent } = workspace.api;
  const {
    getHarnessesQuery,
    getOpenCodeProvidersQuery,
    getLocalModelsQuery,
    getProjectPathStatusQuery,
    refreshHarnesses,
  } = workspace.queries;
  const { useOpenResearchAccount, useAlphaXivMe } = useMachines().owner.queries;
  const accountQuery = useOpenResearchAccount();
  const alphaXivMe = useAlphaXivMe();
  const user = accountQuery.data?.user ?? null;
  // An OpenResearch account without an alphaXiv connection still needs to connect it.
  const connectOnly = !!user && alphaXivMe.error instanceof AlphaXivNotConnectedError;
  const alphaXivLinked = !!user && alphaXivMe.isSuccess;

  const [step, setStep] = useState<0 | 1 | 2>(remote ? 1 : 0);
  useEffect(() => {
    const name = ONBOARDING_STEP_NAMES[step];
    if (name && !remote) captureUiEvent({ name: "onboarding_step_viewed", step: name });
  }, [step]);
  const harnessQuery = useQuery(getHarnessesQuery());
  const providersQuery = useQuery(getOpenCodeProvidersQuery());
  const endpointsQuery = useQuery(getLocalModelsQuery());
  const pathQuery = useQuery({ ...getProjectPathStatusQuery(), enabled: !remote });
  /** OpenCode's install for the free models, which starts by itself while OpenCode is missing and outlives the onboarding. */
  const [freeModels, setFreeModels] = useState<{ installing: boolean; error: string | null }>({ installing: false, error: null });
  const installOutput = useRef("");
  const setupTask = useRef<Promise<Harness> | null>(null);
  const installStarted = useRef(false);
  const harnesses = harnessQuery.data ?? null;
  const gitVersion = pathQuery.data?.gitVersion;
  const gitInstallable = !remote && pathQuery.data?.gitInstallable === true;
  const [finishing, setFinishing] = useState(false);
  const [finishError, setFinishError] = useState<string | null>(null);
  const [finishErrorDetails, setFinishErrorDetails] = useState("");
  /** The `PickerAgent.key` of the agent added here that the first chat starts on. */
  const [pickedKey, setPickedKey] = useState<string | null>(null);
  /** The `PickerAgent.key` of an agent just added, which the first chat starts on once it can. */
  const [pendingKey, setPendingKey] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [gitError, setGitError] = useState(false);

  // Step 1 requires one agent that can start a chat now, and local Git. Failed or
  // inconclusive detection never bypasses either gate — and a harness whose
  // snapshot is still being filled in is inconclusive, not ready.
  const agents = pickerAgents(harnesses ?? [], providersQuery.data ?? [], endpointsQuery.data ?? []);
  const ready = agents.flatMap((agent) => {
    const selection = startingSelection(agent);
    return selection ? [{ agent, selection }] : [];
  });
  const savedHarness = preferredAgent && selectionHarness(harnesses ?? [], preferredAgent);
  const savedAgent = savedHarness && agentFor(agents, savedHarness, preferredAgent.model);
  // The first chat starts on the agent picked while it is ready and on, else on the saved choice, else on the first agent ready.
  const start = (ready.find(({ agent }) => agent.key === pickedKey) ?? ready.find(({ agent }) => agent === savedAgent) ?? ready[0])?.selection;
  const anyAgentReady = ready.length > 0;
  const anyPending = harnesses?.some((h) => h.catalogPending) ?? false;
  const opencode = harnesses?.find((h) => h.id === "opencode" && !h.profile);
  const opencodeMissing = Boolean(opencode && (!opencode.installed || opencode.installBroken));
  // With no agent ready, and none just added or still being checked, the first chat waits on the free models while OpenCode installs.
  const startsOnFreeModels = !start && !pendingKey && !anyPending && !remote && opencodeMissing;
  const gitReady = remote || gitVersion != null;

  // Drops a slow probe whose answer a newer load has already superseded.
  const loadSeq = useRef(0);
  const load = (refresh: boolean, retryRejected = false) => {
    const seq = ++loadSeq.current;
    setChecking(true);
    setGitError(false);
    const fresh = () => seq === loadSeq.current;
    // The agent list reports a failed agent check itself.
    void Promise.allSettled([
      refreshHarnesses(refresh, retryRejected),
      remote ? Promise.resolve() : queryClient.fetchQuery({ ...getProjectPathStatusQuery(), staleTime: refresh ? 0 : 30_000 }),
    ])
      .then(([, gitStatus]) => {
        if (fresh() && gitStatus.status === "rejected") setGitError(true);
      })
      .finally(() => fresh() && setChecking(false));
  };
  useEffect(() => load(false), []);
  const gitInstall = useMutate(mutInstallGit);
  const gitInstallPrompt = (
    <GitInstallPrompt
      pending={gitInstall.isMutating}
      error={gitInstall.error}
      disabled={checking}
      onInstall={() => gitInstall.runWithOptions(workspace, { onSuccessData: () => load(true), onError: () => {} })}
    />
  );
  const readyKeys = ready.map(({ agent }) => agent.key).join("\n");
  useEffect(() => {
    if (!pendingKey || !ready.some(({ agent }) => agent.key === pendingKey)) return;
    setPickedKey(pendingKey);
    setPendingKey(null);
  }, [readyKeys, pendingKey]);
  useEffect(() => setGitError(!remote && pathQuery.isError), [remote, pathQuery.isError, pathQuery.dataUpdatedAt]);
  const installFreeModels = () => {
    if (setupTask.current) return setupTask.current;
    installOutput.current = "";
    installStarted.current = true;
    setFreeModels({ installing: true, error: null });
    const task = (async () => {
      await new Promise<void>((resolve, reject) => {
        const socket = new WebSocket(socketUrl("/api/harnesses/setup?harness=opencode&action=install&trigger=automatic"));
        socket.binaryType = "arraybuffer";
        const decoder = new TextDecoder();
        // ConPTY waits for terminal replies even when installation runs without a visible terminal.
        const terminal = new Xterm();
        terminal.onData((data) => {
          if (socket.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
        });
        let complete = false;
        socket.onmessage = (event) => {
          if (event.data instanceof ArrayBuffer) {
            terminal.write(new Uint8Array(event.data));
            // Keep the latest 64 KiB so noisy installer output cannot grow without bound.
            installOutput.current = (installOutput.current + decoder.decode(event.data, { stream: true })).slice(-65536);
            return;
          }
          if (typeof event.data !== "string") return;
          let value: unknown;
          try { value = JSON.parse(event.data); } catch { return; }
          if (typeof value !== "object" || value === null || !("type" in value)) return;
          if (value.type === "complete") {
            complete = true;
            socket.close();
            resolve();
          } else if (value.type === "error") {
            socket.close();
            reject(new Error("error" in value && typeof value.error === "string" ? value.error : m.harness_setup_failed()));
          }
        };
        socket.onerror = () => { socket.close(); reject(new Error(m.settings_terminal_closed())); };
        socket.onclose = () => {
          terminal.dispose();
          if (!complete) reject(new Error(m.settings_terminal_closed()));
        };
      });
      const detected = await refreshHarnesses(true, true);
      const opencode = detected.find((h) => h.id === "opencode" && h.defaultModel);
      if (!opencode) throw new Error(m.onboarding_opencode_not_ready());
      return opencode;
    })();
    setupTask.current = task;
    task.then(
      () => setFreeModels({ installing: false, error: null }),
      (error: unknown) => {
        setupTask.current = null;
        setFreeModels({ installing: false, error: error instanceof Error ? error.message : String(error) });
      },
    );
    return task;
  };

  useEffect(() => {
    // Wait out a pending snapshot: an install detection still in flight must
    // not read as "not installed" and trigger an unattended setup.
    if (remote || installStarted.current || anyPending || !opencodeMissing) return;
    void installFreeModels().catch(() => {});
  }, [harnesses, remote]);

  const finishOnboarding = async () => {
    if (props.remote) return;
    if (finishing || !gitReady) return;
    setFinishing(true);
    setFinishError(null);
    setFinishErrorDetails("");
    try {
      const installed = startsOnFreeModels ? await installFreeModels() : null;
      const selection = installed?.defaultModel ? selectionFor(installed, installed.defaultModel) : start;
      if (!selection) throw new Error(m.harness_setup_not_ready());
      const completion = await mutCompleteOnboarding.runAsHeadlessPromise(workspace, selection, {});
      props.onDone(completion.project, completion.selection);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setFinishError(message);
      setFinishErrorDetails([installOutput.current, message].filter(Boolean).join("\n\n"));
    } finally {
      setFinishing(false);
    }
  };

  // Sign-in resolves several renders after the click; finish with what is true by then.
  const latestFinish = useRef(finishOnboarding);
  latestFinish.current = finishOnboarding;

  const continueFromAgents = async () => {
    if (!props.remote) { setStep(2); return; }
    if (finishing) return;
    if (!start) { setFinishError(m.harness_setup_not_ready()); return; }
    setFinishing(true);
    setFinishError(null);
    try {
      props.onDone(await mutUpdateUiState.runAsHeadlessPromise(workspace, { preferredAgent: start }, {}));
    } catch (error) {
      setFinishError(error instanceof Error ? error.message : String(error));
    } finally {
      setFinishing(false);
    }
  };
  // An agent the wizard adds is ready when it closes, and an ACP agent once it has been checked.
  const onAdded = (added: AddedAgent) => setPendingKey(added.provider ? `opencode:${added.provider}` : added.profile ?? added.harness);

  const continueHint = checking || (!anyAgentReady && anyPending)
    ? m.onboarding_waiting_tool_checks()
    : !anyAgentReady && !startsOnFreeModels
      ? m.onboarding_sign_in_agent_to_continue()
      : remote ? undefined : gitError
          ? m.onboarding_recheck_git_to_continue()
          : gitVersion === undefined
            ? m.onboarding_waiting_git_check()
            : gitVersion === null
              ? m.onboarding_install_git_to_continue()
              : undefined;

  return (
    <div
      className={`home flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges] bg-background onboarding ${
        step === 0
          ? "[&_.home-inner]:max-w-300 [&_.home-inner]:pt-0 [&_.home-inner]:pb-0"
          : "[&_.home-inner]:max-w-180 [&_.home-inner]:pt-24"
        }`}
    >
      <div
        className={`home-inner max-w-155 my-0 mx-auto ${
          step === 0 ? "px-8 sm:px-12" : "pt-12 px-6 pb-16"
          }`}
      >
        {step === 0 ? (
          <div className="onb-intro flex min-h-dvh flex-col items-center justify-center gap-6 text-center">
            <Hyperspace className="pointer-events-none fixed inset-0 size-full" />
            <div className="onb-intro-brand relative text-6xl font-semibold leading-none tracking-tight animate-[onb-warp-in_700ms_cubic-bezier(0.16,1,0.3,1)_1250ms_both] motion-reduce:animate-none">
              <Wordmark />
            </div>
            <h2 className="onb-title relative m-0 text-2xl font-medium text-muted tracking-tight animate-[onb-welcome-in_700ms_ease-out_1700ms_both] motion-reduce:animate-none">
              {m.onboarding_the_local_first_workspace_for_research_agents()}
            </h2>
            <div className="relative mt-6 animate-[onb-welcome-in_600ms_ease-out_2300ms_both] motion-reduce:animate-none">
              <Button variant="primary" size="large"
                onClick={() => setStep(1)}
                disabled={checking && harnesses === null}
              >
                {checking && harnesses === null ? <Spinner /> : null}{m.onboarding_continue()} <ArrowRight size={20} />
              </Button>
            </div>
          </div>
        ) : step === 1 ? (
          <>
            <StepHeader
              step={remote ? null : m.onboarding_step_1_of_2()}
              title={props.remote ? m.remote_agents_title({ host: ltr(props.host) }) : m.onboarding_agents_title()}
              description={props.remote ? m.remote_agents_description({ host: ltr(props.host) }) : m.onboarding_agents_description()}
            />
            {!remote && (gitVersion === null || gitError) && (
              <div className="onb-git-check mt-7 mb-5.5" role="status" aria-live="polite">
                <LocalGitCard gitVersion={gitVersion} error={gitError} />
                {gitError ? (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>{m.onboarding_retry_connection()}</p>
                ) : gitInstallable ? (
                  gitInstallPrompt
                ) : (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>
                    {m.onboarding_git_is_required_for_local_experiments_install_git()}
                  </p>
                )}
              </div>
            )}
            <AgentList
              remote={false}
              onboarding={{
                host: props.remote ? props.host : undefined,
                onAdded,
                freeModels: { ...freeModels, onInstall: () => void installFreeModels().catch(() => {}) },
              }}
            />
            <div className="onb-actions flex items-center gap-2.5 mt-5.5">
              {!remote && <Button variant="ghost" onClick={() => setStep(0)}>
                <ArrowLeft size={12} /> {m.onboarding_back()}
              </Button>}
              <Button variant="ghost" onClick={() => load(true, true)} disabled={checking}>
                <RefreshCw size={12} className={checking ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.onboarding_re_check()}
              </Button>
              <div className="flex-1" />
              <Button variant="primary"
                onClick={() => void continueFromAgents()}
                disabled={finishing || checking || (!start && !startsOnFreeModels) || !gitReady}
                aria-describedby={continueHint ? elementId("onboarding-continue-hint") : undefined}
              >
                {finishing && <Spinner />}{m.onboarding_continue()} <ArrowRight size={14} />
              </Button>
            </div>
            {finishError && <p role="alert" className="text-sm text-accent-red">{finishError}</p>}
            {continueHint && <p id={elementId("onboarding-continue-hint")} className="text-sm text-subtext">{continueHint}</p>}
          </>
        ) : (
          <div>
            <StepHeader step={m.onboarding_step_2_of_2()} title={m.onboarding_alphaxiv_title()} description={m.onboarding_alphaxiv_description()} />
            <AlphaXivAccountCard
              user={alphaXivLinked ? user : null}
              avatarUrl={alphaXivMe.data?.avatarUrl ?? null}
              connectOnly={connectOnly}
              disabled={accountQuery.isLoading || (!!user && alphaXivMe.isLoading) || finishing}
              onSignedIn={() => void latestFinish.current()}
            />
            <div className="onb-actions flex items-center gap-2.5 mt-5.5">
              <Button variant="ghost" onClick={() => setStep(1)} disabled={finishing}>
                <ArrowLeft size={12} /> {m.onboarding_back()}
              </Button>
              <div className="flex-1" />
              <Button variant="primary"
                onClick={() => void finishOnboarding()}
                disabled={finishing || !gitReady || (!start && !startsOnFreeModels)}
              >
                {finishing ? (
                  <>
                    <Spinner /> {m.onboarding_setting_things_up()}
                  </>
                ) : (
                  <>
                    {alphaXivLinked ? m.onboarding_get_started() : m.onboarding_skip()} <ArrowRight size={14} />
                  </>
                )}
              </Button>
            </div>
            {!start && !startsOnFreeModels && (
              <p className={FINISH_ERROR_CLASS_NAME}>
                {m.onboarding_your_selected_agent_is_no_longer_ready_go()}
              </p>
            )}
            {finishError && (
              <div role="alert" className="mt-8 grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-md border border-danger-notice-border bg-accent-red-subtle px-3 py-2">
                <AlertCircle size={16} className="shrink-0 text-accent-red" />
                <p className="m-0 min-w-0 flex-1 text-sm text-text break-words">{finishError}</p>
                {startsOnFreeModels && (
                  <Button size="small" onClick={() => void finishOnboarding()} disabled={finishing}>
                    {m.app_retry()}
                  </Button>
                )}
                <details className="col-start-2 col-span-2 min-w-0 text-sm text-text">
                  <summary className="cursor-pointer">{m.onboarding_error_details()}</summary>
                  <pre className="mt-2 mb-0 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border border-danger-notice-border bg-background px-2 py-1.5 font-mono text-xs">{finishErrorDetails || finishError}</pre>
                </details>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function selectionFor(harness: Harness, model: string): AgentSelection {
  return {
    harness: harness.id,
    profile: harness.profile?.id ?? null,
    model,
    permissionMode: harness.options?.defaultPermissionMode ?? null,
    reasoningLevel: reasoningFor(harness, model).defaultId,
  };
}

/** What a first chat on `agent` runs: the agent's own default, else the first model the picker lists for it. */
function startingSelection(agent: PickerAgent): AgentSelection | null {
  const offers = agentOffers(agent);
  const [first] = groupModelLines(offers);
  const offer = offers.find(({ model }) => model.id === agent.harness.defaultModel) ?? (first && defaultOffer(first.offers));
  return offer ? selectionFor(agent.harness, offer.model.id) : null;
}

/** What each setup step opens with, the same on every step so nothing jumps between them. */
function StepHeader({ step, title, description }: { step: string | null; title: string; description: string }) {
  return (
    <>
      <div className="mb-5 flex items-center gap-2 text-xl font-medium text-muted">
        <Wordmark />
        {step && <span>{step}</span>}
      </div>
      <h2 className="m-0 mb-2 text-4xl tracking-tight">{title}</h2>
      <p className="m-0 mb-7 text-lg leading-normal text-pretty text-text">{description}</p>
    </>
  );
}

function GitInstallPrompt({
  pending,
  error,
  disabled,
  onInstall,
}: {
  pending: boolean;
  error: unknown;
  disabled: boolean;
  onInstall: () => void;
}) {
  return (
    <div className="flex flex-col items-start gap-2.5">
      <p className={GIT_RETRY_HINT_CLASS_NAME}>{m.onboarding_install_git_for_windows()}</p>
      <Button variant="primary" onClick={onInstall} disabled={pending || disabled}>
        {pending && <Spinner />}
        {pending ? m.onboarding_installing_git() : m.onboarding_install_git()}
      </Button>
      {error != null && <p className="text-accent-red text-sm m-0">{error instanceof Error ? error.message : String(error)}</p>}
    </div>
  );
}

function AlphaXivAccountCard({ user, avatarUrl, connectOnly, disabled, onSignedIn }: {
  user: OpenResearchAccountInfo["user"];
  avatarUrl: string | null;
  connectOnly: boolean;
  disabled: boolean;
  onSignedIn: () => void;
}) {
  const openSignIn = useSignIn();
  const card = useRef<HTMLDivElement>(null);
  const [failedAvatar, setFailedAvatar] = useState<string | null>(null);
  const initial = user && ([user.firstName, user.lastName].filter(Boolean).join(" ") || user.email).trim().charAt(0).toLocaleUpperCase();
  return (
    // Sign-in swaps the button for the account card; the dialog returns focus here.
    <div ref={card} tabIndex={-1} className="outline-none">
      {user ? (
        <div className="flex items-center gap-4 rounded-lg border border-border bg-background px-5 py-4">
          {avatarUrl && avatarUrl !== failedAvatar
            ? <img src={avatarUrl} alt="" className="size-12 shrink-0 rounded-full object-cover" onError={() => setFailedAvatar(avatarUrl)} />
            : <span className="flex size-12 shrink-0 items-center justify-center rounded-full bg-panel text-lg font-medium text-text" aria-hidden="true">{initial}</span>}
          <span className="min-w-0 flex-1 text-base"><OpenResearchAccount user={user} /></span>
          <StatusIndicator tone="success">{m.onboarding_signed_in()}</StatusIndicator>
        </div>
      ) : (
        <Button size="large" className="h-16 w-full justify-center text-lg" disabled={disabled} onClick={() => openSignIn(connectOnly, card.current, onSignedIn)} aria-haspopup="dialog">
          <LitSourceLogo source="alphaxiv" size={24} decorative framed={false} />{m.account_sign_in_alphaxiv()}
        </Button>
      )}
    </div>
  );
}

function LocalGitCard({
  gitVersion,
  error,
}: {
  gitVersion: string | null | undefined;
  error: boolean;
}) {
  return (
    <div className="onb-card flex flex-col gap-1 bg-background border border-border rounded-lg py-4.5 px-5">
      <div className="onb-card-head flex items-center justify-between gap-3">
        <span className="onb-card-name font-semibold text-base">{m.onboarding_local_git()}</span>
        <StatusIndicator tone={gitVersion ? "success" : error || gitVersion === null ? "danger" : "warning"}>
          {gitVersion ? m.onboarding_ready() : error ? m.onboarding_check_failed() : gitVersion === null ? m.onboarding_not_found() : m.onboarding_checking()}
        </StatusIndicator>
      </div>
      {(gitVersion || (!error && gitVersion === undefined)) && (
        <div className={ONB_CARD_META_CLASS_NAME}>{gitVersion ?? m.onboarding_checking_git()}</div>
      )}
    </div>
  );
}
