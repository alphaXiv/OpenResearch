import { useMachines } from "../machineSelection";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { AlphaXivNotConnectedError, OpenResearchAccountError, type RuntimeInfo } from "../api";
import { Menu } from "@base-ui/react/menu";
import { useQuery } from "@tanstack/react-query";
import { useContext, useEffect, useRef, useState } from "react";
import { UserRound } from "lucide-react";
import { type DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { confirmOpenResearchSignOut } from "./confirmOpenResearchSignOut";
import { accountErrorMessage, OpenResearchAccount } from "./OpenResearchAccount";

import { Button, DialogContent, DialogTitle, MenuItem, showAlert, Spinner } from "./ui";

export function isRemoteSession(runtime?: RuntimeInfo | null): boolean {
  if (runtime?.kind === "ssh") return true;
  if (typeof location !== "undefined" && location.hostname !== "127.0.0.1" && location.hostname !== "localhost") {
    return true;
  }
  return false;
}

/** Opens sign-in from anywhere; focus returns to `returnFocus` when the dialog closes. */
export function useSignIn() {
  const workspace = useWorkspace();
  const { owner } = useMachines();
  // Sign-in runs on the laptop's server; only a laptop window that is itself remote must open the browser.
  const remote = isRemoteSession(useQuery(owner.queries.getRuntimeQuery(), owner.queryClient).data);
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
    const inDesktopApp = typeof window !== "undefined" && "__ORX_DESKTOP__" in window;
    const placeholder = remote && !inDesktopApp ? window?.open?.("/remote-launch", "_blank") ?? null : null;
    if (remote && !inDesktopApp && !placeholder) {
      showAlert(m.remote_popup_blocked(), "error");
    }
    showCustomDialog({
      component: SignInDialog,
      props: { connectOnly, returnFocus, initialPlaceholder: placeholder, remote },
      onCancel: () => {
        placeholder?.close();
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
  const { useOpenResearchAccount, useAlphaXivFolders, useSignOutOpenResearch } = owner.queries;
  const openSignIn = useSignIn();
  const { portalContainer } = useContext(WorkspaceDomContext);
  const accountQuery = useOpenResearchAccount();
  const user = accountQuery.data?.user;
  const alphaXivNotConnected = useAlphaXivFolders().error instanceof AlphaXivNotConnectedError;
  const signOut = useSignOutOpenResearch();
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
  const label = user ? name : reason ? stateLabel : accountQuery.isLoading ? m.common_checking() : m.account_sign_in_alphaxiv();
  const chipClass = "flex min-h-[44.5px] min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 py-1.5 text-start hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus";
  const contents = <><span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-panel text-sm font-medium text-text" aria-hidden="true">{user ? name.trim().charAt(0).toLocaleUpperCase() : <UserRound size={16} />}</span><span className="flex min-w-0 flex-col"><span dir="auto" className="truncate text-sm leading-tight">{label}</span>{user && <span className="truncate text-xs leading-tight text-subtext">{reason ? stateLabel : user.alphaxivUrl ? "alphaXiv" : "OpenResearch"}</span>}</span></>;

  return user || reason ? <Menu.Root modal={false} loopFocus>
      <Menu.Trigger ref={chipRef} aria-label={`${label}${user && reason ? ` · ${stateLabel}` : ""} · ${m.account_menu()}`} className={chipClass}>{contents}</Menu.Trigger>
      <Menu.Portal container={portalContainer}><Menu.Positioner side="top" align="start" sideOffset={6} collisionPadding={8} className="z-50">
        <Menu.Popup aria-label={m.account_menu()} className="outline-none max-h-(--available-height) overflow-y-auto w-64 max-w-[calc(100vw-1rem)] rounded-lg border border-border bg-background p-1.5 shadow-menu">
          <div className="flex flex-col gap-1 border-b border-border-variant px-2 py-2 text-sm break-words">{user && <OpenResearchAccount user={user} />}{reason && <span role="status">{reason}</span>}</div>
          {reason && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => void accountQuery.refetch()}>{m.settings_check_again()}</Menu.Item>}
          {user && !accountQuery.isError && alphaXivNotConnected && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => signIn(true)}>{m.library_connect()}</Menu.Item>}
          <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={onDetails}>{m.account_details()}</Menu.Item>
          {user && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} disabled={signOut.isPending} onClick={() => confirmOpenResearchSignOut(workspace.dialogs, signOut.mutateAsync)}>{m.settings_openresearch_sign_out()}</Menu.Item>}
        </Menu.Popup>
      </Menu.Positioner></Menu.Portal>
    </Menu.Root> : <button ref={chipRef} type="button" className={chipClass} disabled={accountQuery.isLoading} onClick={() => signIn(false)}>{contents}</button>;
}

/** sysexits' EX_TEMPFAIL, which `orx login` exits with when OpenResearch couldn't reach alphaXiv. */
const TRY_AGAIN_LATER = 75;

function SignInDialog({ connectOnly, returnFocus, initialPlaceholder, remote, confirm, cancel }: DialogContentProps<void> & {
  connectOnly: boolean;
  returnFocus: HTMLElement | null;
  initialPlaceholder: Window | null;
  remote: boolean;
}) {
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const placeholderRef = useRef<Window | null>(initialPlaceholder);

  const fail = (exitCode?: number) => {
    placeholderRef.current?.close();
    placeholderRef.current = null;
    if (exitCode === TRY_AGAIN_LATER) setError(m.account_alphaxiv_unavailable());
    else setError(connectOnly ? m.account_alphaxiv_not_connected() : m.account_sign_in_failed());
  };

  const retry = () => {
    const inDesktopApp = typeof window !== "undefined" && "__ORX_DESKTOP__" in window;
    const nextPlaceholder = remote && !inDesktopApp ? window?.open?.("/remote-launch", "_blank") ?? null : null;
    if (remote && !inDesktopApp && !nextPlaceholder) {
      showAlert(m.remote_popup_blocked(), "error");
    }
    placeholderRef.current = nextPlaceholder;
    setError(null);
    setAttempt((value) => value + 1);
  };

  const handleCancel = () => {
    placeholderRef.current?.close();
    placeholderRef.current = null;
    cancel();
  };

  return <DialogContent initialFocus={cancelRef} finalFocus={() => returnFocus} className="w-140 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
    <DialogTitle className="mb-4">{connectOnly ? m.library_connect() : m.account_sign_in_alphaxiv()}</DialogTitle>
    {error ? <p role="alert" className="mt-0 mb-3 text-sm text-accent-red">{error}</p> : <div role="status" className="mb-3 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.account_finish_browser()}</div>}
    <LoginSession
      key={attempt}
      connectOnly={connectOnly}
      remote={remote}
      placeholderWindow={placeholderRef.current}
      onComplete={() => confirm()}
      onError={fail}
    />
    <div className="mt-4 flex justify-end gap-2">
      {error && <Button onClick={retry}>{m.app_retry()}</Button>}
      <Button ref={cancelRef} onClick={handleCancel}>{m.settings_page_cancel()}</Button>
    </div>
  </DialogContent>;
}

export function LoginSession({
  connectOnly,
  remote,
  placeholderWindow,
  onComplete,
  onError,
}: {
  connectOnly: boolean;
  remote: boolean;
  placeholderWindow: Window | null;
  onComplete: () => void;
  onError: (exitCode?: number) => void;
}) {
  const { socketUrl } = useMachines().owner;
  const [url, setUrl] = useState<string | null>(null);
  const callbacks = useRef({ onComplete, onError });
  callbacks.current = { onComplete, onError };
  const opened = useRef(false);
  useEffect(() => {
    let socket: WebSocket | undefined;
    let output = "";
    let complete = false;
    const timer = window.setTimeout(() => {
      socket = new WebSocket(socketUrl(connectOnly ? "/api/settings/alphaxiv/login" : "/api/settings/openresearch/login"));
      socket.binaryType = "arraybuffer";
      socket.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
          output += new TextDecoder().decode(event.data);
          const match = output.match(/https?:\/\/[^\s\x1b]+(?=\s)/);
          if (match) {
            const authUrl = match[0];
            setUrl(authUrl);
            if (!opened.current) {
              opened.current = true;
              if (remote) {
                if (placeholderWindow && !placeholderWindow.closed) {
                  placeholderWindow.location.replace(authUrl);
                } else {
                  window?.open?.(authUrl, "_blank");
                }
              }
            }
          }
          return;
        }
        try {
          const value = JSON.parse(event.data);
          if (value.type === "complete") { complete = true; callbacks.current.onComplete(); }
          else if (value.type === "error") { complete = true; setUrl(null); callbacks.current.onError(value.exitCode); }
        } catch { /* Ignore non-control messages. */ }
      };
      socket.onclose = () => { if (!complete) { setUrl(null); callbacks.current.onError(); } };
    }, 0);
    return () => { clearTimeout(timer); if (socket) { socket.onclose = null; socket.close(); } };
  }, [connectOnly, remote, placeholderWindow, socketUrl]);
  return url && <a href={url} target="_blank" rel="noreferrer" className="break-all text-sm text-text underline">{url}</a>;
}
