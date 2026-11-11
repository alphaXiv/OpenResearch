import { useContext, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useMachines } from "../machineSelection";
import { useQuery } from "@tanstack/react-query";
import { SlidersHorizontal, X } from "lucide-react";

import type { DialogContentProps } from "../dialog";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { mutCreateRemoteSession } from "../queries/settings";

import { getThemePreference } from "../theme";
import { ProgressBar } from "./ProgressBar";
import { Button, DialogContent, DialogDescription, DialogTitle, IconButton, Input, Spinner } from "./ui";

export function RemoteHostDialog({
  confirm,
  cancel,
  isDisabled,
  errorMessage,
  onConfigureSsh,
}: DialogContentProps<() => Promise<void>> & {
  onConfigureSsh: () => void;
}) {
  const { id: elementId } = useContext(WorkspaceDomContext);

  const machines = useMachines();
  const { getSshSettingsQuery, listRemoteSessionsQuery } = machines.owner.queries;

  const hostsQuery = useQuery(getSshSettingsQuery(), machines.owner.queryClient);
  const sessionsQuery = useQuery(listRemoteSessionsQuery(), machines.owner.queryClient);
  const hosts = hostsQuery.data?.hosts ?? null;
  const sessions = sessionsQuery.data ?? [];
  const [query, setQuery] = useState("");
  const loadError = !hosts ? hostsQuery.error?.message ?? sessionsQuery.error?.message ?? null : null;
  const [openingHost, setOpeningHost] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  function openRemote(host: string) {
    setOpeningHost(host);
    confirm(async () => {
      try {
        const session = await mutCreateRemoteSession.runAsHeadlessPromise(machines.owner, host, {
          theme: getThemePreference(),
          locale: getLocale(),
        }, {});
        machines.selectRemote(session);
      } finally {
        setOpeningHost(null);
      }
    });
  }

  const filteredHosts = hosts?.filter((host) =>
    host.host.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  const sessionByHost = new Map(sessions.map((session) => [session.host, session]));

  return <DialogContent
      initialFocus={searchRef}
      className="relative flex h-[min(42rem,calc(100vh-2.5rem))] w-160 max-w-full flex-col overflow-hidden p-0"
      aria-labelledby={elementId("remote-host-dialog-title")}
    >
      <IconButton className="absolute end-3.5 top-3.5" aria-label={m.remote_dialog_close()} onClick={cancel} disabled={isDisabled}>
        <X size={16} />
      </IconButton>
      <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
        <DialogTitle id={elementId("remote-host-dialog-title")}>{m.remote_dialog_title()}</DialogTitle>
        <DialogDescription className="mt-2 leading-normal">{m.remote_dialog_description()}</DialogDescription>
        <Input
          ref={searchRef}
          className="mt-4"
          value={query}
          disabled={isDisabled}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={m.remote_search_hosts()}
          aria-label={m.remote_search_hosts()}
        />
        <div role="status">
          {openingHost && <ProgressBar label={m.remote_connecting_title({ host: ltr(openingHost) })} />}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto border-t border-border-variant p-2">
        {errorMessage && <p role="alert" className="m-3 text-sm text-accent-red">{errorMessage}</p>}
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
                disabled={isDisabled}
                onClick={() => openRemote(host.host)}
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
        <Button variant="ghost" className="w-full justify-start text-base font-normal" onClick={() => { cancel(); onConfigureSsh(); }} disabled={isDisabled}>
          <SlidersHorizontal size={16} />
          {m.ssh_configure_hosts()}
        </Button>
      </div>
    </DialogContent>;
}
