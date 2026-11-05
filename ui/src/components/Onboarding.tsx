import type { DialogContentProps } from "../dialog";
import { useContext, useEffect, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { useMachines } from "../machineSelection";
import { useSignIn } from "./AccountChip";
import { OpenResearchAccount } from "./OpenResearchAccount";
import { LitSourceLogo } from "./LitSourceLogo";

import { useMutation, useQuery } from "@tanstack/react-query";
import { Terminal as Xterm } from "@xterm/xterm";

import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { AlertCircle, ArrowLeft, ArrowRight, Plus, RefreshCw, Terminal, TriangleAlert } from "lucide-react";
import { Wordmark } from "./Wordmark";

import {
  type HarnessSetupCommands,
  reasoningFor,
  type AgentSelection,
  type Harness,
  type HarnessId,
  type OnboardingStep,
  type Project,
  type UiState,
  AlphaXivNotConnectedError,
  type OpenResearchAccount as OpenResearchAccountInfo,
} from "../api";
import { renderNote } from "./agentNote";
import { claudeProviderLabel } from "./claudeProvider";
import { HarnessLogo } from "./HarnessLogo";
import { AcpAuthDialog } from "./AcpAuthDialog";
import { AcpHarnessDialog } from "./AcpHarnessDialog";
import { HarnessSetupDialog } from "./HarnessSetupDialog";

import { Hyperspace } from "./Hyperspace";
import { Button, DialogContent, DialogTitle, LoadingRow, Spinner, StatusIndicator, Tooltip, type StatusTone } from "./ui";

const ONB_GATE_HINT_CLASS_NAME = [
  "onb-gate-hint text-base font-medium leading-normal text-text",
  "onb-agent-hint mt-0 mx-0 mb-2.5",
].join(" ");

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

  const { socketUrl } = useWorkspace();
  const element = useRef<HTMLDivElement>(null);
  const { showCustomDialog } = useWorkspace().dialogs;

  const { completeOnboarding, captureUiEvent, installGit, updateUiState } = useWorkspace().api;
  const {
    getPublicNetworkQuery,
    getHarnessesQuery,
    getProjectPathStatusQuery,
    getHarnessSetupCommandsQuery,
    refreshHarnesses,
  } = useWorkspace().queries;
  const { queryClient, workspaceScope, isCurrentScope } = useWorkspace();
  const { useOpenResearchAccount, useAlphaXivMe } = useMachines().owner.queries;
  const accountQuery = useOpenResearchAccount();
  const alphaXivMe = useAlphaXivMe();
  const user = accountQuery.data?.user ?? null;
  // An OpenResearch account without an alphaXiv connection still needs to connect it.
  const connectOnly = !!user && alphaXivMe.error instanceof AlphaXivNotConnectedError;
  const alphaXivLinked = !!user && alphaXivMe.isSuccess;

  const completeOnboardingMutation = useMutation({ mutationFn: completeOnboarding });

  const [step, setStep] = useState<0 | 1 | 2>(remote ? 1 : 0);
  useEffect(() => {
    const name = ONBOARDING_STEP_NAMES[step];
    if (name && !remote) captureUiEvent({ name: "onboarding_step_viewed", step: name });
  }, [step]);
  const harnessQuery = useQuery(getHarnessesQuery());
  const warnedDatacenter = useRef(false);
  const [checkingNetwork, setCheckingNetwork] = useState(false);
  const pathQuery = useQuery({ ...getProjectPathStatusQuery(), enabled: !remote });
  const setupCommands = useQuery(getHarnessSetupCommandsQuery());
  const addedAcpHarness = useRef<HarnessId | null>(null);
  const [automaticSetup, setAutomaticSetup] = useState(false);
  const installSocket = useRef<WebSocket | null>(null);
  const installOutput = useRef("");
  const setupTask = useRef<Promise<Harness> | null>(null);
  const automaticSetupStarted = useRef(false);
  useEffect(() => {
    const screen = element.current;
    return () => queueMicrotask(() => { if (!screen?.isConnected) installSocket.current?.close(); });
  }, []);
  const harnesses = harnessQuery.data ?? null;
  const gitVersion = pathQuery.data?.gitVersion;
  const gitInstallable = !remote && pathQuery.data?.gitInstallable === true;
  const [finishing, setFinishing] = useState(false);
  const [finishError, setFinishError] = useState<string | null>(null);
  const [finishErrorDetails, setFinishErrorDetails] = useState("");
  const [preferredHarness, setPreferredHarness] = useState<HarnessId | null>(null);
  const [checking, setChecking] = useState(false);
  // Per-probe, not one shared flag: a git failure must not put a connectivity
  // error on the harness gate it has nothing to do with — or worse, hide the
  // actionable "sign in" hint behind it.
  const [harnessError, setHarnessError] = useState(false);
  const [gitError, setGitError] = useState(false);

  // Step 1 requires one genuinely usable harness and local Git. Failed or
  // inconclusive detection never bypasses either gate — and a harness whose
  // snapshot is still being filled in is inconclusive, not ready.
  const anyAgentReady = harnesses?.some((h) => h.agentReady && !h.catalogPending) ?? false;
  const anyPending = harnesses?.some((h) => h.catalogPending) ?? false;
  const gitReady = remote || gitVersion != null;

  // Drops a slow probe whose answer a newer load has already superseded.
  const loadSeq = useRef(0);
  const load = (refresh: boolean, retryRejected = false) => {
    const seq = ++loadSeq.current;
    setChecking(true);
    setHarnessError(false);
    setGitError(false);
    const fresh = () => seq === loadSeq.current;
    void Promise.allSettled([
      refreshHarnesses(refresh, retryRejected),
      remote ? Promise.resolve() : queryClient.fetchQuery({ ...getProjectPathStatusQuery(), staleTime: refresh ? 0 : 30_000 }),
    ])
      .then(([harness, gitStatus]) => {
        if (!fresh()) return;
        if (harness.status === "rejected") {
          setHarnessError(true);
        }
        if (gitStatus.status === "rejected") {
          setGitError(true);
        }
      })
      .finally(() => fresh() && setChecking(false));
  };
  useEffect(() => load(false), []);
  const installGitMutation = useMutation({ mutationFn: installGit, onSuccess: () => load(true) });
  const gitInstallPrompt = (
    <GitInstallPrompt
      pending={installGitMutation.isPending}
      error={installGitMutation.error}
      disabled={checking}
      onInstall={() => installGitMutation.mutate()}
    />
  );
  useEffect(() => {
    if (harnesses === null) return;
    const ready = harnesses.filter((h) => h.agentReady && !h.catalogPending);
    const added = ready.find((h) => h.id === addedAcpHarness.current);
    if (added) {
      addedAcpHarness.current = null;
      setPreferredHarness(added.id);
      return;
    }
    setPreferredHarness((current) => {
      if (current && ready.some((h) => h.id === current)) return current;
      const saved = preferredAgent && ready.find((h) => h.id === preferredAgent.harness);
      return saved?.id ?? ready[0]?.id ?? null;
    });
  }, [harnesses, preferredAgent]);
  useEffect(() => setHarnessError(harnessQuery.isError), [harnessQuery.isError, harnessQuery.dataUpdatedAt]);
  useEffect(() => setGitError(!remote && pathQuery.isError), [remote, pathQuery.isError, pathQuery.dataUpdatedAt]);
  const startAutomaticSetup = () => {
    if (setupTask.current) return setupTask.current;
    installOutput.current = "";
    automaticSetupStarted.current = true;
    setAutomaticSetup(true);
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
        installSocket.current = socket;
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
          installSocket.current = null;
          if (!complete) reject(new Error(m.settings_terminal_closed()));
        };
      });
      const detected = await refreshHarnesses(true, true);
      const opencode = detected.find((h) => h.id === "opencode" && h.agentReady);
      if (!opencode) throw new Error(m.onboarding_opencode_not_ready());
      setPreferredHarness("opencode");
      return opencode;
    })();
    setupTask.current = task;
    void task.catch(() => {});
    return task;
  };

  useEffect(() => {
    // Wait out a pending snapshot: an install detection still in flight must
    // not read as "nothing installed" and trigger an unattended setup.
    if (remote || automaticSetupStarted.current || !harnesses?.length || harnesses.some((h) => h.catalogPending) || !harnesses.every((h) => !h.installed && !h.installBroken)) return;
    void startAutomaticSetup();
  }, [harnesses, remote]);

  useEffect(() => {
    if (step === 1 && automaticSetup) setStep(2);
  }, [step, automaticSetup]);

  const continueFromWelcome = () => {
    setStep(automaticSetup ? 2 : 1);
  };

  const finishOnboarding = async (mode: "complete" | "setup" = "complete") => {
    if (props.remote) return;
    if (finishing || (mode === "complete" && !gitReady)) return;
    setFinishing(true);
    setFinishError(null);
    setFinishErrorDetails("");
    try {
      const harness = automaticSetup
        ? await startAutomaticSetup()
        : harnesses?.find((item) => item.id === preferredHarness && item.agentReady && !item.catalogPending);
      if (!harness) throw new Error(m.harness_setup_not_ready());
      if (mode === "setup") return;
      const selection = selectionFor(harness, harness.models[0]?.id ?? null);
      const completion = await completeOnboardingMutation.mutateAsync(selection);
      props.onDone(completion.project, completion.selection);
    } catch (error) {
      if (automaticSetup) setupTask.current = null;
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

  const openAuth = (harness: Harness) => showCustomDialog({
    component: AcpAuthDialog,
    props: { remote, harness, onReady: (ready) => setPreferredHarness(ready.id) },
    onConfirm: () => {},
    onCancel: "close",
  });
  const openSetup = async (harness: Harness) => {
    const commands = setupCommands.data?.[harness.id];
    if (!commands) return;
    const open = () => showCustomDialog({
      component: HarnessSetupDialog,
      props: { harness, commands, onReady: (ready) => setPreferredHarness(ready.id), onAuth: openAuth },
      onConfirm: () => {},
      onCancel: "close",
    });
    if (remote && harness.id === "claude-code" && harness.authMethod !== "apiKey" && !warnedDatacenter.current) {
      const screen = element.current;
      setCheckingNetwork(true);
      const network = await queryClient.fetchQuery(getPublicNetworkQuery()).catch(() => null);
      if (!isCurrentScope(workspaceScope()) || !screen?.isConnected) return;
      setCheckingNetwork(false);
      if (network?.isDatacenter) {
        warnedDatacenter.current = true;
        showCustomDialog({
          component: ClaudeDatacenterWarning,
          props: { host: props.host, ip: network.ip },
          onConfirm: open,
          onCancel: "close",
        });
        return;
      }
    }
    open();
  };
  const continueFromAgents = async () => {
    if (!props.remote) { setStep(2); return; }
    const harness = harnesses?.find((h) => h.id === preferredHarness && h.agentReady && !h.catalogPending);
    if (finishing) return;
    if (!harness) { setFinishError(m.harness_setup_not_ready()); return; }
    setFinishing(true);
    setFinishError(null);
    try {
      const selection = selectionFor(harness, harness.models[0]?.id ?? null);
      props.onDone(await updateUiState({ preferredAgent: selection }));
    } catch (error) {
      setFinishError(error instanceof Error ? error.message : String(error));
    } finally {
      setFinishing(false);
    }
  };
  const openAcp = () => showCustomDialog({
    component: AcpHarnessDialog,
    props: {
      onSaved: (id) => {
        const ready = queryClient.getQueryData(getHarnessesQuery().queryKey)?.find((h) => h.id === id && h.agentReady && !h.catalogPending);
        addedAcpHarness.current = ready ? null : id;
        if (ready) setPreferredHarness(ready.id);
        load(true, true);
      },
    },
    onConfirm: (save) => save(),
    onCancel: "close",
  });

  const continueHint = checking || (!anyAgentReady && anyPending)
    ? m.onboarding_waiting_tool_checks()
    : !anyAgentReady
      ? m.onboarding_sign_in_agent_to_continue()
      : preferredHarness === null
        ? m.onboarding_choose_preferred_agent()
        : remote ? undefined : gitError
          ? m.onboarding_recheck_git_to_continue()
          : gitVersion === undefined
            ? m.onboarding_waiting_git_check()
            : gitVersion === null
              ? m.onboarding_install_git_to_continue()
              : undefined;

  return (
    <div ref={element}
      className={`home flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges] bg-background onboarding ${
        step === 0
          ? "[&_.home-inner]:max-w-300 [&_.home-inner]:pt-0 [&_.home-inner]:pb-0"
          : step === 2
            ? "[&_.home-inner]:max-w-180 [&_.home-inner]:pt-0 [&_.home-inner]:pb-0"
            : "[&_.home-inner]:max-w-140 [&_.home-inner]:pt-24"
        }`}
    >
      <div
        className={`home-inner max-w-155 my-0 mx-auto ${
          step === 0 ? "px-8 sm:px-12" : step === 2 ? "px-6" : "pt-12 px-6 pb-16"
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
                onClick={continueFromWelcome}
                disabled={checking && harnesses === null}
              >
                {checking && harnesses === null ? <Spinner /> : null}{m.onboarding_continue()} <ArrowRight size={20} />
              </Button>
            </div>
          </div>
        ) : step === 1 ? (
          <>
            <div className="onb-eyebrow mb-4.5 flex items-center gap-2 text-xl font-medium text-muted">
              <Wordmark />
              {!remote && <span>{m.onboarding_step_1_of_2()}</span>}
            </div>
            <h2 className="onb-title mt-0 mx-0 mb-1.5 text-3xl tracking-tight">{props.remote ? m.remote_agents_title({ host: ltr(props.host) }) : m.onboarding_choose_a_coding_agent()}</h2>
            <p className="onb-sub text-text text-base leading-normal mt-0 mx-0 mb-5.5 max-w-120">{props.remote ? m.remote_agents_description({ host: ltr(props.host) }) : m.onboarding_open_research_uses_a_coding_agent_already_installed()}</p>
            {harnesses !== null && !anyAgentReady && !anyPending && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                {m.onboarding_sign_in_to_at_least_one_agent_to()}
              </p>
            )}
            {harnesses !== null && anyAgentReady && preferredHarness === null && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                {m.onboarding_choose_a_coding_agent_to_continue()}
              </p>
            )}
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
            <div className="onb-cards flex flex-col gap-3.5">
              {harnesses !== null ? (
                harnesses.filter((h) => !h.id.startsWith("acp:") || h.installed).map((h) => (
                  <AgentCard
                    key={h.id}
                    h={h}
                    selected={preferredHarness === h.id}
                    onSelect={() => { addedAcpHarness.current = null; setPreferredHarness(h.id); }}
                    commands={setupCommands.data?.[h.id]}
                    setupPending={h.id === "claude-code" && checkingNetwork}
                    onSetup={() => void openSetup(h)}
                    onAuth={() => openAuth(h)}
                  />
                ))
              ) : harnessError ? (
                // Never a spinner next to an error — detection isn't running.
                <div className={ONB_CARD_META_CLASS_NAME}>{m.onboarding_retry_connection()}</div>
              ) : (
                <LoadingRow className="py-2">
                  <Spinner /> {m.onboarding_detecting_claude_code_codex_open_code()}
                </LoadingRow>
              )}
              <Button variant="ghost" className="self-start" onClick={() => openAcp()} aria-haspopup="dialog"><Plus size={14} />{m.acp_add()}</Button>
            </div>
            {setupCommands.isError && <p className={ONB_CARD_META_CLASS_NAME}>
              {m.harness_setup_load_failed()} <Button variant="ghost" onClick={() => void setupCommands.refetch()}>{m.app_retry()}</Button>
            </p>}
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
                disabled={finishing || checking || !anyAgentReady || preferredHarness === null || !gitReady}
                aria-describedby={continueHint ? elementId("onboarding-continue-hint") : undefined}
              >
                {finishing && <Spinner />}{m.onboarding_continue()} <ArrowRight size={14} />
              </Button>
            </div>
            {finishError && <p role="alert" className="text-sm text-accent-red">{finishError}</p>}
            {continueHint && <p id={elementId("onboarding-continue-hint")} className="text-sm text-subtext">{continueHint}</p>}
          </>
        ) : (
          <div className="flex min-h-dvh flex-col justify-center py-16">
            <div className="onb-eyebrow mb-6 flex items-center gap-2 text-xl font-medium text-muted">
              <Wordmark />
              {!automaticSetup && <span>{m.onboarding_step_2_of_2()}</span>}
            </div>
            <h2 className="onb-title mt-0 mx-0 mb-3 text-5xl tracking-tight">{m.onboarding_alphaxiv_title()}</h2>
            <p className="onb-sub text-text text-lg leading-normal mt-0 mx-0 mb-8 max-w-150">{m.onboarding_alphaxiv_description()}</p>
            <AlphaXivAccountCard
              user={alphaXivLinked ? user : null}
              avatarUrl={alphaXivMe.data?.avatarUrl ?? null}
              connectOnly={connectOnly}
              disabled={accountQuery.isLoading || (!!user && alphaXivMe.isLoading) || finishing}
              onSignedIn={() => void latestFinish.current()}
            />
            {automaticSetup && !gitReady && (
              <div className="mt-5" role="status">
                <LocalGitCard gitVersion={gitVersion} error={gitError} />
                {gitError ? (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>{m.onboarding_retry_connection()}</p>
                ) : gitInstallable ? (
                  gitInstallPrompt
                ) : (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>{m.onboarding_git_is_required_for_local_experiments_install_git()}</p>
                )}
                <Button className="mt-2.5" onClick={() => load(true, true)} disabled={checking}>{m.onboarding_re_check()}</Button>
              </div>
            )}
            <div className="onb-actions flex items-center gap-2.5 mt-5.5">
              <Button variant="ghost" onClick={() => setStep(automaticSetup ? 0 : 1)} disabled={finishing}>
                <ArrowLeft size={12} /> {m.onboarding_back()}
              </Button>
              <div className="flex-1" />
              <Button variant="primary"
                onClick={() => void finishOnboarding()}
                disabled={finishing || !gitReady || (!automaticSetup && preferredHarness === null)}
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
            {preferredHarness === null && !automaticSetup && (
              <p className={FINISH_ERROR_CLASS_NAME}>
                {m.onboarding_your_selected_agent_is_no_longer_ready_go()}
              </p>
            )}
            {finishError && (
              <div role="alert" className="mt-8 grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-md border border-danger-notice-border bg-accent-red-subtle px-3 py-2">
                <AlertCircle size={16} className="shrink-0 text-accent-red" />
                <p className="m-0 min-w-0 flex-1 text-sm text-text break-words">{finishError}</p>
                {automaticSetup && (
                  <Button size="small" onClick={() => void finishOnboarding(gitReady ? "complete" : "setup")} disabled={finishing}>
                    {m.app_retry()}
                  </Button>
                )}
                {automaticSetup && (
                  <Button variant="ghost" size="small" className="col-start-2 justify-self-start" disabled={finishing} onClick={() => {
                    setAutomaticSetup(false);
                    setFinishError(null);
                    setStep(1);
                  }}>{m.onboarding_choose_another_agent()}</Button>
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

/** Agent notes carry the command to run in backticks (`claude auth login`) —
 * render those spans as code so they read as something to type, not prose. */
function agentBadge(h: Harness): { tone: StatusTone; label: string } {
  // A snapshot answer still being filled in — "checking" rather than a badge
  // the background pass may revoke.
  if (h.adapterMissing) return { tone: "warning", label: m.acp_adapter_required() };
  if (h.catalogPending) return { tone: "warning", label: m.onboarding_checking() };
  if (h.id.startsWith("acp:") && h.authState === "needsLogin") return { tone: "warning", label: m.acp_auth_setup_needed() };
  if (h.authCheckFailed) return { tone: "warning", label: m.onboarding_unable_to_verify() };
  if (h.id.startsWith("acp:") && h.authState === "ready") return { tone: "success", label: m.acp_credentials_configured() };
  if (h.agentReady) return { tone: "success", label: h.authMethod === "thirdParty" || claudeProviderLabel(h) ? m.settings_page_ready_to_use() : h.authMethod === "local" || !h.authenticated ? m.onboarding_ready() : m.onboarding_signed_in() };
  if (!h.installed) return { tone: "neutral", label: m.onboarding_not_detected() };
  if (h.installBroken) return { tone: "warning", label: m.onboarding_install_broken() };
  if (h.authMethod === "local") return { tone: "warning", label: m.onboarding_server_unavailable() };
  // A config fault reports `unsupported`, but no update repairs it; the note
  // carries the actual repair, so the badge must not promise an update.
  if (h.needsConfigRepair || h.authState === "unknown") return { tone: "warning", label: m.onboarding_unable_to_verify() };
  if (h.id === "claude-code" && h.authState === "needsLogin" && !h.loginEligible) return { tone: "warning", label: m.onboarding_unable_to_verify() };
  if (h.authState === "unsupported") return { tone: "warning", label: m.onboarding_update_required() };
  if (h.installed) return { tone: "warning", label: m.onboarding_not_signed_in() };
  return { tone: "neutral", label: m.onboarding_not_detected() };
}

function selectionFor(harness: Harness, model: string | null): AgentSelection {
  return {
    harness: harness.id,
    model,
    permissionMode: harness.options?.defaultPermissionMode ?? null,
    reasoningLevel: reasoningFor(harness, model).defaultId,
  };
}

function AgentLogo({ harness, logo }: { harness: HarnessId; logo?: string }) {
  return <HarnessLogo harness={harness} logo={logo} size={26} />;
}

function AgentCard({
  h,
  selected,
  onSelect,
  commands,
  onSetup,
  onAuth,
  setupPending = false,
}: {
  h: Harness;
  selected: boolean;
  onSelect: () => void;
  commands?: HarnessSetupCommands;
  setupPending?: boolean;
  onSetup: () => void;
  onAuth: () => void;
}) {
  // needsConfigRepair means no install/update/login command can fix this state
  // (an environment credential overriding the saved login, a database the CLI
  // will not open). Offering one sends the user through a command that
  // provably cannot help; the agentNote below carries the actual repair.
  const canLogin = h.id === "claude-code" ? Boolean(h.loginEligible) : h.authMethod !== "local" && h.authMethod !== "apiKey" && (h.authState === "needsLogin" || h.authState === "unknown");
  const canSetup = (!h.id.startsWith("acp:") || h.adapterMissing) && !h.needsConfigRepair && !h.catalogPending && (!h.installed || h.installBroken || h.authState === "unsupported" || canLogin);
  const showSetupAction = !h.agentReady && canSetup;
  const showStatusDot = h.adapterMissing || (canSetup && (!h.installed || (!h.agentReady && h.authState === "needsLogin")));
  const badge = agentBadge(h);
  const visibleBadge: { tone: StatusTone; label: string } = selected && !h.authCheckFailed
    ? { tone: "success", label: m.onboarding_selected() }
    : badge;
  const version = h.version?.replace(/\s*\(.*\)$/, "");
  const meta = [
    h.id === "opencode" && h.account !== "opencode" && h.account,
    h.id === "opencode" && h.plan,
  ]
    .filter(Boolean)
    .join(" · ");
  const head = (
    <div className="onb-card-head flex flex-wrap items-center justify-between gap-3">
      <span className="onb-card-identity flex items-center gap-3 min-w-0">
        <AgentLogo harness={h.id} logo={h.logo} />
        <span className="flex flex-col gap-1">
          <span className="onb-card-name flex flex-wrap items-center gap-2 text-base font-semibold tracking-tight">
            {h.name}
            {h.id.startsWith("acp:") && !h.adapterMissing && <span className="text-sm font-normal text-subtext">{m.acp_via()}</span>}
            {showStatusDot && (
              <StatusIndicator tone={badge.tone} className="font-normal">{badge.label}</StatusIndicator>
            )}
          </span>
          {!showStatusDot && !h.id.startsWith("acp:") && showSetupAction && <StatusIndicator tone={visibleBadge.tone}>{visibleBadge.label}</StatusIndicator>}
        </span>
      </span>
      {showSetupAction ? (
        <Tooltip interactive content={m.harness_setup_opens_terminal()} className="rounded-sm">
          <Button size="small" onClick={onSetup} disabled={!commands || setupPending} aria-haspopup="dialog" aria-description={m.harness_setup_opens_terminal()}>
            {setupPending ? <Spinner /> : <Terminal size={14} />}
            {h.adapterMissing ? m.acp_install_adapter() : !h.installed || h.installBroken ? m.harness_setup_install() : h.authState === "unsupported" ? m.harness_setup_update() : m.harness_setup_login()}
          </Button>
        </Tooltip>
      ) : h.id.startsWith("acp:") && !h.agentReady && !h.catalogPending ? (
        <Button variant="ghost" size="small" onClick={onAuth} aria-haspopup="dialog">
          <StatusIndicator tone={visibleBadge.tone}>{visibleBadge.label}</StatusIndicator>
        </Button>
      ) : (
        <StatusIndicator tone={visibleBadge.tone}>{visibleBadge.label}</StatusIndicator>
      )}
    </div>
  );
  // An unready agent can't be selected — render it as a plain container, not a
  // disabled button, so the copy button on its `agentNote` command stays live.
  if (!h.agentReady) {
    return (
      <div className="onb-card flex flex-col gap-2.5 bg-background border border-border rounded-lg py-4 px-4 onb-agent-choice w-full text-inherit [font:inherit] text-start transition-[border-color,box-shadow] duration-120 ease-standard [button&]:cursor-pointer [button&:hover]:border-muted [&.selected]:border-text [&.selected]:shadow-selected">
        {head}
        {h.authState === "unsupported" && version && (
          <div className={ONB_CARD_META_CLASS_NAME}>{version}</div>
        )}
        {!h.id.startsWith("acp:") && !showSetupAction && h.agentNote && (
          <div className={`${ONB_CARD_META_CLASS_NAME} [&_code]:whitespace-pre-wrap break-words`}>{renderNote(h.agentNote)}</div>
        )}
        {!h.installed && h.needsConnectionAdapter && <p className={ONB_CARD_META_CLASS_NAME}>{m.acp_adapter_help({ name: h.name })}</p>}
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`onb-card flex flex-col gap-2.5 bg-background border border-border rounded-lg py-4 px-4 onb-agent-choice w-full text-inherit [font:inherit] text-start transition-[border-color,box-shadow] duration-120 ease-standard [button&]:cursor-pointer [button&:hover]:border-muted [&.selected]:border-text [&.selected]:shadow-selected${selected ? " selected" : ""}`}
      aria-pressed={selected}
      onClick={onSelect}
    >
      {head}
      {h.id !== "opencode" && !h.id.startsWith("acp:") && (
        <div className="onb-card-detail flex items-center gap-1.5 text-sm">
          {h.accountLoading ? <><Spinner /> {m.onboarding_loading_account()}</> : <>
            {claudeProviderLabel(h) ?? h.account ?? (h.authMethod === "apiKey" ? m.onboarding_api_key() : null)}
            {h.plan ? ` · ${h.plan}` : ""}
          </>}
        </div>
      )}
      {meta && <div className={ONB_CARD_META_CLASS_NAME}>{meta}</div>}
      {h.authCheckFailed && h.agentNote && <div className={ONB_CARD_META_CLASS_NAME}>{renderNote(h.agentNote)}</div>}
    </button>
  );
}

function GitInstallPrompt({
  pending,
  error,
  disabled,
  onInstall,
}: {
  pending: boolean;
  error: Error | null;
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
      {error && <p className="text-accent-red text-sm m-0">{error.message}</p>}
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

function ClaudeDatacenterWarning({ host, ip, confirm, cancel }: DialogContentProps<void> & { host: string; ip: string }) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  return <DialogContent role="alertdialog" initialFocus={cancelRef} className="w-140 max-w-full p-6">
    <TriangleAlert size={36} className="mb-4 text-accent-amber" aria-hidden="true" />
    <DialogTitle className="mb-3 text-2xl text-accent-amber">{m.remote_claude_datacenter_title()}</DialogTitle>
    <p className="mb-5 text-base text-text">{m.remote_claude_datacenter_description({ host: ltr(host), ip: ltr(ip) })}</p>
    <div className="flex justify-end gap-2">
      <Button ref={cancelRef} variant="primary" onClick={cancel}>{m.common_cancel()}</Button>
      <Button onClick={() => confirm()}>{m.remote_claude_datacenter_continue()}</Button>
    </div>
  </DialogContent>;
}
