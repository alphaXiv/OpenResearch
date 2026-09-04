import { useEffect, useRef, useState, type ReactNode } from "react";
import App from "./App";
import {
  disconnectCurrentRemote,
  getRuntime,
  getRemoteStopPreview,
  installRemoteSession,
  reconnectCurrentRemote,
  startCurrentRemoteHost,
  stopCurrentRemoteHost,
  type RemoteInstallPaths,
  type RemoteStopPreview,
  type RuntimeInfo,
} from "./api";
import { ltr } from "./i18n";
import { setLocale } from "./locale";
import { m } from "./paraglide/messages.js";
import { isLocale } from "./paraglide/runtime.js";
import { setThemePreference } from "./theme";
import { Button, Input, showAlert, Spinner } from "./components/ui";
import { RemoteStopDialog } from "./components/RemoteStopDialog";
import { SshConnectTerminal } from "./components/SshConnectTerminal";

const REMOTE_FAVICON =
  "data:image/svg+xml," +
  encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="8" fill="#3b82f6"/><path d="M15.375 16.782v63.843a4 4 0 0 0 4 4h63.843c3.564 0 5.348-4.309 2.829-6.828L22.203 13.953c-2.52-2.52-6.828-.735-6.828 2.829" fill="#fff"/></svg>');

function setFavicon(remote: boolean) {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (link) link.href = remote ? REMOTE_FAVICON : "/favicon.svg";
}

function hasStoredPreference(key: string) {
  try {
    return localStorage.getItem(key) !== null;
  } catch {
    return false;
  }
}

function applyRemotePreferences(runtime: RuntimeInfo) {
  if (runtime.kind !== "ssh") return;
  const { theme, locale } = runtime.session.uiPreferences;
  if (
    !hasStoredPreference("orx:theme") &&
    (theme === "light" || theme === "dark" || theme === "system")
  ) {
    setThemePreference(theme);
  }
  if (!hasStoredPreference("orx:locale") && locale && isLocale(locale)) {
    setLocale(locale);
  }
}

function needsInteractiveSsh(error: string) {
  return error.includes("ssh ") && error.includes("failed");
}

function GatewayUnavailable({ host, overlay = false }: { host: string; overlay?: boolean }) {
  return (
    <div className={overlay ? "w-full max-w-2xl" : "app flex h-full items-center justify-center bg-background p-6"}>
      <section className="w-full max-w-2xl rounded-xl border border-border bg-background p-7 shadow-modal">
        <h1 id="remote-setup-title" className="m-0 text-2xl font-semibold text-text">
          {m.remote_disconnected_title({ host: ltr(host) })}
        </h1>
        <p className="mt-2 mb-0 text-base text-text">{m.offline_banner_disconnected()}</p>
        <p className="mt-2 mb-0 text-sm text-subtext">{m.remote_gateway_unavailable()}</p>
      </section>
    </div>
  );
}

function RemoteSetup({
  runtime,
  overlay = false,
  retriedInteractiveError,
  setRetriedInteractiveError,
}: {
  runtime: Extract<RuntimeInfo, { kind: "ssh" }>;
  overlay?: boolean;
  retriedInteractiveError: string | null;
  setRetriedInteractiveError: (error: string | null) => void;
}) {
  const { session } = runtime;
  const [paths, setPaths] = useState<RemoteInstallPaths | null>(session.installPaths);
  const [busy, setBusy] = useState(false);
  const [stopPreview, setStopPreview] = useState<RemoteStopPreview | null>(null);

  useEffect(() => setPaths(session.installPaths), [
    session.installPaths?.binary,
    session.installPaths?.database,
    session.installPaths?.cache,
  ]);

  async function install() {
    if (!paths) return;
    setBusy(true);
    try {
      await installRemoteSession(paths);
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  async function reconnect(afterInteractiveSsh = false) {
    setRetriedInteractiveError(afterInteractiveSsh ? session.error : null);
    setBusy(true);
    try {
      await reconnectCurrentRemote();
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    setBusy(true);
    try {
      await disconnectCurrentRemote();
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  async function prepareStopHost() {
    setBusy(true);
    try {
      setStopPreview(await getRemoteStopPreview());
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  async function stopHost() {
    if (!stopPreview) return;
    setBusy(true);
    try {
      await stopCurrentRemoteHost(stopPreview);
      setStopPreview(null);
    } catch (error) {
      setStopPreview(null);
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  async function startNewHost() {
    setBusy(true);
    try {
      await startCurrentRemoteHost();
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setBusy(false);
    }
  }

  const installing = session.status === "applying" || busy;
  const needsInstall = session.status === "needsInstall";
  const needsUpdate = session.status === "needsUpdate";
  const showInstallPaths = paths && needsInstall;
  const working = ["connecting", "applying", "reconnecting"].includes(session.status);
  const showSshTerminal = session.status === "disconnected" &&
    session.error !== null &&
    needsInteractiveSsh(session.error) &&
    retriedInteractiveError !== session.error &&
    !session.canStartNewHost;
  const title = needsInstall
    ? m.remote_install_title()
    : needsUpdate
      ? m.remote_update_title()
      : session.status === "applying"
        ? m.remote_applying_title({ host: ltr(session.host) })
        : session.status === "reconnecting"
          ? m.remote_reconnecting_title({ host: ltr(session.host) })
          : session.status === "disconnected"
            ? session.error
              ? m.remote_failed_title({ host: ltr(session.host) })
              : session.canStartNewHost
              ? m.remote_host_stopped_title({ host: ltr(session.host) })
              : m.remote_disconnected_title({ host: ltr(session.host) })
            : m.remote_connecting_title({ host: ltr(session.host) });
  const description = session.error ?? (session.canStartNewHost
    ? m.remote_host_stopped_description()
    : needsInstall
    ? m.remote_not_installed_description({
        user: ltr(session.user ?? ""),
        host: ltr(session.host),
      })
    : needsUpdate
      ? m.remote_update_description({ host: ltr(session.host) })
      : session.status === "applying"
        ? m.remote_applying_description()
        : session.status === "reconnecting"
          ? m.remote_reconnecting_description()
          : session.status === "disconnected"
            ? m.remote_disconnected_description()
            : m.remote_connecting_description());

  return (
    <>
      <main className={overlay ? "w-full max-w-2xl" : "app flex h-full items-center justify-center bg-background p-6"}>
      <section className="w-full max-w-2xl rounded-xl border border-border bg-background p-7 shadow-modal">
        <div className="flex items-start gap-3">
          {working && <Spinner className="mt-2" />}
          <div className="min-w-0 flex-1">
            <h1 id="remote-setup-title" className="m-0 text-2xl font-semibold text-text">
              {title}
            </h1>
            {!showSshTerminal && <p className="mt-2 mb-0 text-base text-text">{description}</p>}
          </div>
        </div>

        {showSshTerminal && (
          <SshConnectTerminal
            host={session.host}
            backend="ssh"
            path="/_orx/ssh/connect"
            onComplete={() => void reconnect(true)}
          />
        )}

        {showInstallPaths && (
          <div className="mt-6 grid gap-4 border-t border-border-variant pt-5">
            <p className="m-0 text-sm text-subtext">{m.remote_install_location()}</p>
            {([
              ["binary", m.remote_install_binary()],
              ["database", m.remote_install_database()],
              ["cache", m.remote_install_cache()],
            ] as const).map(([key, label]) => (
              <label key={key} className="grid gap-1 text-sm font-medium text-subtext">
                {label}
                <Input
                  value={paths[key]}
                  onChange={(event) => setPaths({ ...paths, [key]: event.target.value })}
                  disabled={installing}
                  dir="ltr"
                />
              </label>
            ))}
            {!session.error && (
              <div className="flex justify-end pt-1">
                <Button variant="primary" disabled={installing} onClick={() => void install()}>
                  {installing ? <><Spinner /> {m.remote_installing()}</> : needsUpdate ? m.remote_update() : m.settings_page_install()}
                </Button>
              </div>
            )}
          </div>
        )}

        {needsUpdate && paths && (
          <div className="mt-6 flex justify-end">
            {!session.error && (
              <Button variant="primary" disabled={installing} onClick={() => void install()}>
                {installing ? <><Spinner /> {m.remote_updating()}</> : m.remote_update()}
              </Button>
            )}
          </div>
        )}

        {session.status === "disconnected" && (
          <div className="mt-6 flex justify-end border-t border-border-variant pt-5">
            {session.canStartNewHost ? (
              <Button variant="primary" disabled={busy} onClick={() => void startNewHost()}>
                {busy ? <Spinner /> : null}
                {session.error ? m.remote_reconnect() : m.remote_start_new_host()}
              </Button>
            ) : (
              <Button variant="primary" disabled={busy} onClick={() => void reconnect()}>
                {busy ? <Spinner /> : null}
                {m.remote_reconnect()}
              </Button>
            )}
          </div>
        )}
        {(session.status === "connecting" || session.status === "reconnecting") && (
          <div className="mt-6 flex justify-end border-t border-border-variant pt-5">
            <Button disabled={busy} onClick={() => void disconnect()}>
              {m.remote_disconnect()}
            </Button>
          </div>
        )}
        {needsUpdate && session.error && (
          <div className="mt-6 flex justify-end gap-2 border-t border-border-variant pt-5">
            <Button disabled={busy} onClick={() => void disconnect()}>
              {m.remote_disconnect()}
            </Button>
            {session.installPaths !== null &&
              (session.dashboardProtocol === null || session.dashboardProtocol < runtime.dashboardProtocol) && (
                <Button variant="primary" disabled={busy} onClick={() => void reconnect()}>
                  {busy ? <Spinner /> : null}
                  {m.remote_check_again()}
                </Button>
              )}
            {session.installPaths === null && session.dashboardProtocol !== null && session.dashboardProtocol < runtime.dashboardProtocol && (
              <Button variant="danger" disabled={busy} onClick={() => void prepareStopHost()}>
                {busy ? <Spinner /> : null}
                {m.remote_stop_host()}
              </Button>
            )}
          </div>
        )}
        {needsInstall && session.error && (
          <div className="mt-6 flex justify-end">
            <Button variant="primary" disabled={busy} onClick={() => void reconnect()}>
              {busy ? <Spinner /> : null}
              {m.remote_check_again()}
            </Button>
          </div>
        )}
        </section>
      </main>
      {stopPreview && (
        <RemoteStopDialog
          host={session.host}
          preview={stopPreview}
          currentClientAttached={false}
          stopping={busy}
          onClose={() => {
            if (!busy) setStopPreview(null);
          }}
          onConfirm={() => void stopHost()}
        />
      )}
    </>
  );
}

function RemoteOverlay({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => ref.current?.focus(), []);

  return (
    <div
      ref={ref}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="remote-setup-title"
      tabIndex={-1}
      className="absolute inset-0 z-100 flex items-center justify-center bg-modal-backdrop p-6"
    >
      {children}
    </div>
  );
}

export function RuntimeRoot() {
  const launchPlaceholder = location.pathname === "/remote-launch";
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const everConnected = useRef(false);
  const workspaceMounted = useRef(false);
  const preferencesApplied = useRef(false);
  const [retriedInteractiveError, setRetriedInteractiveError] = useState<string | null>(null);

  useEffect(() => {
    if (launchPlaceholder) return;
    let active = true;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await getRuntime();
        if (!active) return;
        if (next.kind === "ssh") {
          if (!preferencesApplied.current) {
            preferencesApplied.current = true;
            applyRemotePreferences(next);
          }
          if (next.session.status === "connected") {
            everConnected.current = true;
            workspaceMounted.current = true;
            setRetriedInteractiveError(null);
          } else if (next.session.status === "disconnected" && next.session.error === null) {
            workspaceMounted.current = false;
          }
        }
        setRuntime((current) => JSON.stringify(current) === JSON.stringify(next) ? current : next);
        setError(null);
        if (next.kind === "ssh") timer = window.setTimeout(() => void refresh(), 2_000);
      } catch (cause) {
        if (active) {
          setError(cause instanceof Error ? cause.message : String(cause));
          timer = window.setTimeout(() => void refresh(), 2_000);
        }
      }
    };
    void refresh();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [launchPlaceholder]);

  useEffect(() => {
    const remote = runtime?.kind === "ssh";
    setFavicon(remote);
    if (remote && (!everConnected.current || (runtime.session.status === "disconnected" && !runtime.session.error))) {
      document.title = "OpenResearch";
    }
  }, [runtime]);

  if (launchPlaceholder) {
    return (
      <main className="app flex h-full items-center justify-center gap-3 bg-background text-base text-text">
        <Spinner /> {m.remote_preparing()}
      </main>
    );
  }
  if (!runtime) {
    return (
      <main className="app flex h-full items-center justify-center gap-3 bg-background text-base text-text">
        {error
          ? <><span>{error}</span><Button onClick={() => location.reload()}>{m.app_retry()}</Button></>
          : <Spinner />}
      </main>
    );
  }
  if (runtime.kind === "local") return <App runtime={runtime} />;
  const keepWorkspace = workspaceMounted.current &&
    (runtime.session.status !== "disconnected" || runtime.session.error !== null);
  if (!keepWorkspace && runtime.session.status !== "connected") {
    return error
      ? <GatewayUnavailable host={runtime.session.host} />
      : <RemoteSetup
          runtime={runtime}
          retriedInteractiveError={retriedInteractiveError}
          setRetriedInteractiveError={setRetriedInteractiveError}
        />;
  }
  const showOverlay = runtime.session.status !== "connected" || error !== null;
  return (
    <div className="relative h-full">
      <div className="h-full" inert={showOverlay}>
        <App runtime={runtime} />
      </div>
      {showOverlay && (
        <RemoteOverlay>
          {error ? (
            <GatewayUnavailable host={runtime.session.host} overlay />
          ) : (
            <RemoteSetup
              runtime={runtime}
              overlay
              retriedInteractiveError={retriedInteractiveError}
              setRetriedInteractiveError={setRetriedInteractiveError}
            />
          )}
        </RemoteOverlay>
      )}
    </div>
  );
}
