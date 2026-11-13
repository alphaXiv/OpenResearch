import { useContext, useId, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useMachines } from "../machineSelection";
import { useQuery } from "@tanstack/react-query";
import { useMutate } from "@clo/react-mutation";
import { Server, Trash2, X } from "lucide-react";

import type { SshHost } from "../api";
import type { createDialogs, DialogContentProps } from "../dialog";
import { autoDir } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { mutAddRemoteHost, mutCreateRemoteSession, mutRemoveRemoteHost } from "../queries/settings";

import { getThemePreference } from "../theme";
import { ProgressBar } from "./ProgressBar";
import { Button, DialogContent, DialogDescription, DialogTitle, IconButton, Input, Spinner, Tooltip } from "./ui";

export function showRemoteHostDialog(dialogs: ReturnType<typeof createDialogs>) {
  dialogs.showCustomDialog({
    component: RemoteHostDialog,
    props: {},
    onConfirm: (openRemote: () => Promise<void>) => openRemote(),
    onCancel: "close",
  });
}

/** `user@hostname:port` when it says more than the alias does. */
export function sshAddress(host: SshHost): string | null {
  const address = `${host.user ? `${host.user}@` : ""}${host.hostname ?? host.host}${host.port ? `:${host.port}` : ""}`;
  return address === host.host ? null : address;
}

function RemoteHostDialog({
  confirm,
  cancel,
  isDisabled,
  errorMessage,
}: DialogContentProps<() => Promise<void>>) {
  const { id: elementId } = useContext(WorkspaceDomContext);
  const suggestionsId = useId();

  const machines = useMachines();
  const owner = machines.owner;
  const { getSshSettingsQuery, listRemoteHostsQuery, listRemoteSessionsQuery } = owner.queries;

  const hostsQuery = useQuery(listRemoteHostsQuery(), owner.queryClient);
  const sessionsQuery = useQuery(listRemoteSessionsQuery(), owner.queryClient);
  const sshConfigQuery = useQuery(getSshSettingsQuery(), owner.queryClient);
  const remove = useMutate(mutRemoveRemoteHost);
  const hosts = hostsQuery.data ?? null;
  const sessions = sessionsQuery.data ?? [];
  const sessionByHost = new Map(sessions.map((session) => [session.host, session]));
  const suggestions = (sshConfigQuery.data?.hosts ?? [])
    .filter((host) => !hosts?.some((saved) => saved.host === host.host));
  const [command, setCommand] = useState("");
  const [openingHost, setOpeningHost] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  function open(target: (() => Promise<string>) | string) {
    confirm(async () => {
      try {
        const host = typeof target === "string" ? target : await target();
        setOpeningHost(host);
        const session = await mutCreateRemoteSession.runAsHeadlessPromise(owner, host, {
          theme: getThemePreference(),
          locale: getLocale(),
        }, {});
        machines.selectRemote(session);
      } finally {
        setOpeningHost(null);
      }
    });
  }

  function openCompute() {
    cancel();
    machines.selectLocal();
    owner.events.requestComputeView();
  }

  return <DialogContent
      initialFocus={inputRef}
      className="relative flex max-h-[min(42rem,calc(100vh-2.5rem))] w-160 max-w-full flex-col overflow-hidden p-0"
      aria-labelledby={elementId("remote-host-dialog-title")}
    >
      <IconButton className="absolute end-3.5 top-3.5" aria-label={m.remote_dialog_close()} onClick={cancel} disabled={isDisabled}>
        <X size={16} />
      </IconButton>
      <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
        <DialogTitle id={elementId("remote-host-dialog-title")}>{m.remote_dialog_title()}</DialogTitle>
        <DialogDescription className="mt-2 leading-normal">{m.remote_hosts_description()}</DialogDescription>
        <form
          className="mt-4 flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            const typed = command.trim();
            if (!typed) return;
            open(async () => {
              const host = await mutAddRemoteHost.runAsHeadlessPromise(owner, typed, {});
              setCommand("");
              return host;
            });
          }}
        >
          <Input
            ref={inputRef}
            value={command}
            disabled={isDisabled}
            onChange={(event) => setCommand(event.target.value)}
            placeholder={m.remote_add_placeholder()}
            aria-label={m.remote_add_label()}
            list={suggestionsId}
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
          />
          <datalist id={suggestionsId}>
            {suggestions.map((host) => <option key={host.host} value={host.host} />)}
          </datalist>
          <Button type="submit" variant="primary" disabled={isDisabled || !command.trim()}>{m.settings_connect()}</Button>
        </form>
        <div role="status">
          {openingHost && <ProgressBar label={m.remote_connecting_title({ host: autoDir(openingHost) })} />}
        </div>
        {errorMessage && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto border-t border-border-variant p-2">
        {hostsQuery.error ? (
          <p role="alert" className="m-3 text-sm text-accent-red">{hostsQuery.error.message}</p>
        ) : hosts === null ? (
          <div className="flex items-center gap-2 p-3 text-sm text-subtext"><Spinner /> {m.remote_hosts_loading()}</div>
        ) : hosts.length === 0 ? (
          <p className="m-3 text-sm text-subtext">{m.remote_hosts_empty()}</p>
        ) : (
          <ul className="m-0 list-none p-0">
            {hosts.map((host) => {
              const session = sessionByHost.get(host.host);
              const address = sshAddress(host);
              return (
                <li key={host.host} className="group flex items-center gap-1">
                  <Button
                    variant="ghost"
                    className="h-auto min-w-0 flex-1 justify-start py-2 text-base font-normal"
                    disabled={isDisabled}
                    onClick={() => open(host.host)}
                  >
                    <Server size={16} className="flex-none text-subtext" />
                    <span className="min-w-0 flex-1 text-start">
                      <span className="block truncate">{host.host}</span>
                      {address && <span className="block truncate text-sm text-subtext" dir="ltr">{address}</span>}
                    </span>
                    {openingHost === host.host ? (
                      <Spinner />
                    ) : session ? (
                      <span className="text-sm text-subtext">{m.remote_open()}</span>
                    ) : null}
                  </Button>
                  {!session && (
                    <Tooltip interactive content={m.remote_remove_host({ host: autoDir(host.host) })}>
                      <IconButton
                        className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                        aria-label={m.remote_remove_host({ host: autoDir(host.host) })}
                        disabled={isDisabled}
                        onClick={() => remove.run(owner, host.host)}
                      >
                        <Trash2 size={14} />
                      </IconButton>
                    </Tooltip>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-border-variant px-6 py-3">
        <p className="m-0 min-w-0 flex-1 text-sm text-subtext">{m.remote_compute_note()}</p>
        <Button variant="ghost" size="small" onClick={openCompute} disabled={isDisabled}>{m.remote_open_compute()}</Button>
      </div>
    </DialogContent>;
}
