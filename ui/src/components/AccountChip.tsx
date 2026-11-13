import { useMachines } from "../machineSelection";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { AlphaXivNotConnectedError, OpenResearchAccountError } from "../api";
import { Menu } from "@base-ui/react/menu";
import { useContext, useEffect, useRef, useState } from "react";
import { UserRound } from "lucide-react";
import { Terminal as Xterm } from "@xterm/xterm";
import { type DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { confirmOpenResearchSignOut } from "./confirmOpenResearchSignOut";
import { mutSignOutOpenResearch } from "../queries/settings";
import { accountErrorMessage, OpenResearchAccount } from "./OpenResearchAccount";

import { Button, DialogContent, DialogTitle, MenuItem, Spinner } from "./ui";

/**
 * A browser on another machine than the server opens alphaXiv itself, in a tab
 * reserved during the click so the pop-up blocker allows it.
 */
function reserveTab(): Window | null {
  if ("__ORX_DESKTOP__" in window || ["127.0.0.1", "localhost", "[::1]"].includes(location.hostname)) return null;
  return window.open("/remote-launch", "_blank");
}

/** Opens sign-in from anywhere; focus returns to `returnFocus` when the dialog closes. */
export function useSignIn() {
  const workspace = useWorkspace();
  const { owner } = useMachines();
  const { queryClient, workspaceKey } = owner;
  const { invalidateFamilies } = owner.invalidation;
  const { showCustomDialog } = workspace.dialogs;
  function refreshAccount() {
    invalidateFamilies(["getOpenResearchAccount", "getOpenResearchSettings", "getComputeSettings", "getProjectLink"]);
    void queryClient.invalidateQueries({ queryKey: workspaceKey("alphaXivFolders") });
    void queryClient.invalidateQueries({ queryKey: workspaceKey("alphaXivPapers") });
    void queryClient.invalidateQueries({ queryKey: workspaceKey("alphaXivMe") });
  }

  return (connectOnly: boolean, returnFocus: HTMLElement | null, onSignedIn?: () => void) => {
    const tab = { current: reserveTab() };
    showCustomDialog({
      component: SignInDialog,
      props: { connectOnly, returnFocus, tab },
      onCancel: () => {
        tab.current?.close();
        refreshAccount();
      },
      onConfirm: () => {
        window.ipc?.postMessage("window:focus");
        refreshAccount();
        onSignedIn?.();
      },
    });
  };
}

export function AccountChip({ onDetails }: { onDetails: () => void }) {
  const workspace = useWorkspace();
  const { owner } = useMachines();
  const { useOpenResearchAccount, useAlphaXivFolders } = owner.queries;
  const openSignIn = useSignIn();
  const { portalContainer } = useContext(WorkspaceDomContext);
  const accountQuery = useOpenResearchAccount();
  const user = accountQuery.data?.user;
  const alphaXivNotConnected = useAlphaXivFolders().error instanceof AlphaXivNotConnectedError;
  const chipRef = useRef<HTMLButtonElement>(null);
  const focusAfterLogin = useRef(false);
  const name = user ? [user.firstName, user.lastName].filter(Boolean).join(" ") || user.email : "";

  // Signing in swaps the plain button for the menu trigger, so focus moves to the new element.
  const signIn = (connectOnly: boolean) => openSignIn(connectOnly, chipRef.current, () => { focusAfterLogin.current = true; });

  useEffect(() => {
    if (user && focusAfterLogin.current) { chipRef.current?.focus(); focusAfterLogin.current = false; }
  }, [user]);

  const reason = accountErrorMessage(accountQuery.error);
  const stateLabel = accountQuery.error instanceof OpenResearchAccountError && accountQuery.error.kind === "offline" ? m.account_offline_label() : m.account_error_label();
  const label = user ? name : reason ? stateLabel : accountQuery.isLoading ? m.common_checking() : m.settings_sign_in();
  const chipClass = "flex min-h-[44.5px] min-w-0 grow basis-36 items-center gap-2 rounded-md px-1.5 py-1.5 text-start hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus";
  const contents = <><span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-panel text-sm font-medium text-text" aria-hidden="true">{user ? name.trim().charAt(0).toLocaleUpperCase() : <UserRound size={16} />}</span><span className="flex min-w-0 flex-col"><span dir="auto" className="truncate text-sm leading-tight">{label}</span>{user && <span className="truncate text-xs leading-tight text-subtext">{reason ? stateLabel : user.alphaxivUrl ? "alphaXiv" : "OpenResearch"}</span>}</span></>;

  return user || reason ? <Menu.Root modal={false} loopFocus>
      <Menu.Trigger ref={chipRef} aria-label={`${label}${user && reason ? ` · ${stateLabel}` : ""} · ${m.account_menu()}`} className={chipClass}>{contents}</Menu.Trigger>
      <Menu.Portal container={portalContainer}><Menu.Positioner side="top" align="start" sideOffset={6} collisionPadding={8} className="z-50">
        <Menu.Popup aria-label={m.account_menu()} className="outline-none max-h-(--available-height) overflow-y-auto w-64 max-w-[calc(100vw-1rem)] rounded-lg border border-border bg-background p-1.5 shadow-menu">
          <div className="flex flex-col gap-1 border-b border-border-variant px-2 py-2 text-sm break-words">{user && <OpenResearchAccount user={user} />}{reason && <span role="status">{reason}</span>}</div>
          {reason && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => void accountQuery.refetch()}>{m.settings_check_again()}</Menu.Item>}
          {user && !accountQuery.isError && alphaXivNotConnected && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => signIn(true)}>{m.library_connect()}</Menu.Item>}
          <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={onDetails}>{m.account_details()}</Menu.Item>
          {user && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => confirmOpenResearchSignOut(workspace.dialogs, () => mutSignOutOpenResearch.runAsHeadlessPromise(owner, {}))}>{m.settings_openresearch_sign_out()}</Menu.Item>}
        </Menu.Popup>
      </Menu.Positioner></Menu.Portal>
    </Menu.Root> : <button ref={chipRef} type="button" className={chipClass} disabled={accountQuery.isLoading} onClick={() => signIn(false)}>{contents}</button>;
}

/** sysexits' EX_TEMPFAIL, which `orx login` exits with when OpenResearch couldn't reach alphaXiv. */
const TRY_AGAIN_LATER = 75;

interface Failure {
  /** `orx login`'s exit code, from OpenResearch sign-in. */
  exitCode?: number;
  /** The server's message, from connecting alphaXiv. */
  error?: string;
}

/** `tab` is shared with the dialog's cancel, which closes whichever tab is reserved then. */
function SignInDialog({ connectOnly, returnFocus, tab, confirm, cancel }: DialogContentProps<void> & {
  connectOnly: boolean;
  returnFocus: HTMLElement | null;
  tab: { current: Window | null };
}) {
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  const fail = ({ exitCode, error }: Failure) => {
    tab.current?.close();
    tab.current = null;
    if (connectOnly) setError(error ?? m.account_alphaxiv_not_connected());
    else setError(exitCode === TRY_AGAIN_LATER ? m.account_alphaxiv_unavailable() : m.account_sign_in_failed());
  };

  const retry = () => {
    tab.current = reserveTab();
    setError(null);
    setAttempt((value) => value + 1);
  };

  return <DialogContent initialFocus={cancelRef} finalFocus={() => returnFocus} className="w-140 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
    <DialogTitle className="mb-4">{connectOnly ? m.library_connect() : m.account_sign_in_alphaxiv()}</DialogTitle>
    {error
      ? <p role="alert" className="mt-0 mb-3 text-sm text-accent-red">{error}</p>
      : <LoginSession key={attempt} connectOnly={connectOnly} tab={tab.current} onComplete={() => confirm()} onError={fail} />}
    <div className="mt-4 flex justify-end gap-2">
      {error && <Button onClick={retry}>{m.app_retry()}</Button>}
      <Button ref={cancelRef} onClick={cancel}>{m.settings_page_cancel()}</Button>
    </div>
  </DialogContent>;
}

/** Where the user approves the sign-in; a `code` means on alphaXiv's page for it rather than the browser the server opened. */
interface Prompt {
  url: string;
  code?: string;
}

export function LoginSession({
  connectOnly,
  tab,
  onComplete,
  onError,
}: {
  connectOnly: boolean;
  tab: Window | null;
  onComplete: () => void;
  onError: (failure: Failure) => void;
}) {
  const { socketUrl } = useMachines().owner;
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const callbacks = useRef({ onComplete, onError });
  callbacks.current = { onComplete, onError };
  useEffect(() => {
    let socket: WebSocket | undefined;
    let output = "";
    let opened = false;
    let finished = false;
    const open = (url: string) => {
      if (opened) return;
      opened = true;
      if (tab && !tab.closed) tab.location.replace(url);
      else window.open(url, "_blank");
    };
    const show = (next: Prompt) => {
      setPrompt(next);
      if (next.code) open(next.url);
      else tab?.close();
    };
    // ConPTY holds `orx login`'s output until a terminal answers its queries.
    const terminal = new Xterm();
    terminal.onData((data) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
    });
    const timer = window.setTimeout(() => {
      socket = new WebSocket(socketUrl(connectOnly ? "/api/settings/alphaxiv/login" : "/api/settings/openresearch/login"));
      socket.binaryType = "arraybuffer";
      // ConPTY hard-wraps at the terminal's width, which would split the sign-in URL.
      socket.onopen = () => socket?.send(JSON.stringify({ type: "resize", cols: 1000, rows: 24 }));
      socket.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
          terminal.write(new Uint8Array(event.data));
          output += new TextDecoder().decode(event.data);
          const url = output.match(/https?:\/\/[^\s\x1b]+(?=[\s\x1b])/)?.[0];
          if (url) show({ url, code: output.match(/approve the code (\S+):/)?.[1] });
          return;
        }
        let value;
        try { value = JSON.parse(event.data); } catch { return; }
        if (value.type === "authUrl" && /^https?:\/\//.test(value.url)) {
          show({ url: value.url, code: value.code ?? undefined });
        } else if (value.type === "complete") {
          finished = true;
          callbacks.current.onComplete();
        } else if (value.type === "error") {
          finished = true;
          callbacks.current.onError({ exitCode: value.exitCode, error: value.error });
        }
      };
      socket.onclose = () => { if (!finished) callbacks.current.onError({}); };
    }, 0);
    return () => { clearTimeout(timer); terminal.dispose(); if (socket) { socket.onclose = null; socket.close(); } };
  }, [connectOnly, tab, socketUrl]);

  if (prompt?.code) {
    return <>
      <div role="status" className="mb-3 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.account_waiting_approval()}</div>
      <p className="m-0 text-sm text-subtext">{m.account_approve_code()}</p>
      <p dir="ltr" className="my-3 font-mono text-2xl font-medium tracking-widest text-text">{prompt.code}</p>
      <a href={prompt.url} target="_blank" rel="noreferrer" className="text-sm text-text underline">{m.account_open_alphaxiv()}</a>
    </>;
  }
  return <>
    <div role="status" className="mb-3 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.account_finish_browser()}</div>
    {prompt && <a href={prompt.url} target="_blank" rel="noreferrer" className="break-all text-sm text-text underline">{prompt.url}</a>}
  </>;
}
