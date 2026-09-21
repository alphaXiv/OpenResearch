import { useMutation, useQuery } from "@tanstack/react-query";
import { SlidersHorizontal, X } from "lucide-react";
import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { createRemoteSession } from "../api";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { getSshHostsQuery, listRemoteSessionsQuery } from "../queries/settings";
import { getThemePreference } from "../theme";
import { useDialogFocus } from "./useDialogFocus";
import { Button, IconButton, Input, showAlert, Spinner } from "./ui";

export function RemoteHostDialog({
  onClose,
  onConfigureSsh,
}: {
  onClose: () => void;
  onConfigureSsh: () => void;
}) {
  const createRemoteSessionMutation = useMutation({ mutationFn: (args: Parameters<typeof createRemoteSession>) => createRemoteSession(...args) });

  const hostsQuery = useQuery(getSshHostsQuery());
  const sessionsQuery = useQuery(listRemoteSessionsQuery());
  const hosts = hostsQuery.data ?? null;
  const sessions = sessionsQuery.data ?? [];
  const [query, setQuery] = useState("");
  const loadError = !hosts ? hostsQuery.error?.message ?? sessionsQuery.error?.message ?? null : null;
  const [openingHost, setOpeningHost] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  useDialogFocus(dialogRef, onClose);

  async function openRemote(host: string) {
    const remoteWindow = window.open("/remote-launch", "_blank");
    if (!remoteWindow) {
      showAlert(m.remote_popup_blocked(), "error");
      return;
    }
    setOpeningHost(host);
    try {
      const session = await createRemoteSessionMutation.mutateAsync([host, {
        theme: getThemePreference(),
        locale: getLocale(),
      }]);
      remoteWindow.location.replace(session.gatewayUrl);
      onClose();
    } catch (error) {
      remoteWindow.close();
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setOpeningHost(null);
    }
  }

  const filteredHosts = hosts?.filter((host) =>
    host.host.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  const sessionByHost = new Map(sessions.map((session) => [session.host, session]));

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="relative flex h-[min(42rem,calc(100vh-2.5rem))] w-160 max-w-full flex-col overflow-hidden rounded-xl border border-border bg-background shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="remote-host-dialog-title"
        tabIndex={-1}
      >
        <IconButton className="absolute end-3.5 top-3.5" aria-label={m.remote_dialog_close()} onClick={onClose}>
          <X size={16} />
        </IconButton>
        <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
          <h2 id="remote-host-dialog-title" className="m-0 text-xl font-medium">{m.remote_dialog_title()}</h2>
          <p className="mt-2 mb-0 text-sm leading-normal text-subtext">{m.remote_dialog_description()}</p>
          <Input
            data-initial-focus
            className="mt-4"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={m.remote_search_hosts()}
            aria-label={m.remote_search_hosts()}
          />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto border-t border-border-variant p-2">
          {loadError ? (
            <p className="m-3 text-sm text-accent-red">{loadError}</p>
          ) : hosts === null ? (
            <div className="flex items-center gap-2 p-3 text-sm text-subtext"><Spinner /> {m.settings_page_reading_ssh_config()}</div>
          ) : filteredHosts?.length === 0 ? (
            <p className="m-3 text-sm text-subtext">{m.remote_no_matching_hosts()}</p>
          ) : (
            filteredHosts?.map((host) => {
              const session = sessionByHost.get(host.host);
              return (
                <Button
                  key={host.host}
                  variant="ghost"
                  className="w-full justify-start text-base font-normal"
                  disabled={openingHost === host.host}
                  onClick={() => void openRemote(host.host)}
                >
                  <span className="min-w-0 flex-1 truncate text-start">{host.host}</span>
                  {openingHost === host.host ? (
                    <Spinner />
                  ) : session ? (
                    <span className="text-sm text-subtext">{m.remote_open()}</span>
                  ) : null}
                </Button>
              );
            })
          )}
        </div>
        <div className="shrink-0 border-t border-border-variant p-2">
          <Button variant="ghost" className="w-full justify-start text-base font-normal" onClick={onConfigureSsh}>
            <SlidersHorizontal size={15} />
            {m.ssh_configure_hosts()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
