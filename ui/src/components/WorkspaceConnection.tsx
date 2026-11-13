import { useContext, useRef, useState } from "react";
import { useMutate } from "@clo/react-mutation";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { useMachines } from "../machineSelection";
import { RemoteHostDialog } from "./RemoteHostDialog";
import { showSshConfigDialog } from "./SshConfigDialog";
import { DirectionProvider } from "@base-ui/react/direction-provider";

import { Check, ChevronsUpDown, Info, Unplug } from "lucide-react";
import { type RemoteSessionStatus, type RuntimeInfo } from "../api";
import { autoDir, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutDisconnectRemote, mutStopRemoteHost } from "../queries/settings";
import { Menu } from "@base-ui/react/menu";
import { RemoteIcon } from "./RemoteIcon";

import { RemoteStopDialog } from "./RemoteStopDialog";
import { MenuItem, showAlert, Spinner, Tooltip } from "./ui";

function connection(status: RemoteSessionStatus) {
  if (status === "connected") return { state: "connected", label: m.remote_machine_connected() } as const;
  if (status === "connecting" || status === "reconnecting" || status === "applying") return { state: "connecting", label: m.remote_machine_connecting() } as const;
  return { state: "disconnected", label: m.remote_machine_disconnected() } as const;
}

export function WorkspaceConnection({
  runtime,
  corner = false,
}: {
  runtime: RuntimeInfo;
  corner?: boolean;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const machines = useMachines();
  const workspace = useWorkspace();
  const { showCustomDialog } = workspace.dialogs;
  const openHosts = () =>
    showCustomDialog({
      component: RemoteHostDialog,
      props: {
        onConfigureSsh: () =>
          showSshConfigDialog(workspace.dialogs, openHosts, machines.owner),
      },
      onConfirm: (openRemote) => openRemote(),
      onCancel: "close",
    });
  const label =
    runtime.kind === "local" ? m.projects_local() : runtime.session.host;
  const activeSession = machines.sessions.find((session) => session.id === machines.activeId);
  const current = runtime.kind === "ssh" ? connection(activeSession?.status ?? runtime.session.status) : null;
  const machine = runtime.kind === "ssh" ? autoDir(label) : label;
  const caption = current?.state === "connected" ? null : current?.label;
  const triggerName = caption
    ? m.remote_machine_switcher_status({ machine, status: caption })
    : m.remote_machine_switcher({ machine });
  const triggerShape = corner ? "rounded-none px-2.5 py-1.5" : "min-h-9 rounded-md px-2 py-1";
  const triggerTone = current
    ? "border-accent-blue-border bg-accent-blue-subtle text-accent-blue-strong hover:border-accent-blue"
    : "border-border bg-background text-text hover:bg-surface";

  const { getRemoteStopPreview } = workspace.api;

  const disconnect = useMutate(mutDisconnectRemote);
  const [loadingStopPreview, setLoadingStopPreview] = useState(false);
  const [open, setOpen] = useState(false);
  const switching = useRef(false);

  return (
    <>
      <DirectionProvider
        direction={
          typeof document !== "undefined" &&
          document.documentElement.dir === "rtl"
            ? "rtl"
            : "ltr"
        }
      >
        <Menu.Root
          open={open}
          onOpenChange={(open) => {
            if (open) switching.current = false;
            setOpen(open);
          }}
          modal={false}
        >
          <div
            className={
              corner
                ? "workspace-connection-corner fixed bottom-0 start-0 z-30"
                : "relative flex min-w-0 max-w-full"
            }
          >
            <Menu.Portal container={portalContainer}>
              <Menu.Positioner
                side="top"
                align="start"
                sideOffset={6}
                collisionPadding={8}
                // Above the z-100 SSH repair overlay, which keeps this switcher reachable.
                className="z-110"
              >
                <Menu.Popup
                  finalFocus={() => !switching.current}
                  className="outline-none max-h-(--available-height) overflow-y-auto option-menu w-80 max-w-(--available-width) rounded-lg border border-border bg-background p-1.5 shadow-menu"
                >
                  {machines.canSwitch && (
                    <>
                      <Menu.RadioGroup value={machines.activeId}>
                        <Menu.RadioItem
                          nativeButton
                          value="local"
                          closeOnClick
                          render={
                            <MenuItem className="data-highlighted:bg-surface" />
                          }
                          onClick={() => {
                            switching.current = machines.activeId !== "local";
                            setOpen(false);
                            machines.selectLocal();
                          }}
                        >
                          <span>{m.projects_local()}</span>
                          <Menu.RadioItemIndicator>
                            <Check size={14} />
                          </Menu.RadioItemIndicator>
                        </Menu.RadioItem>
                        {machines.sessions.map((session) => (
                          <Menu.RadioItem
                            nativeButton
                            key={session.id}
                            value={session.id}
                            closeOnClick
                            render={
                              <MenuItem className="data-highlighted:bg-surface" />
                            }
                            onClick={() => {
                              switching.current =
                                machines.activeId !== session.id;
                              setOpen(false);
                              machines.selectRemote(session);
                            }}
                          >
                            <span className="min-w-0 flex-1 truncate">
                              {session.host}
                            </span>
                            <span className="text-xs text-subtext">
                              {connection(session.status).label}
                            </span>
                            <Menu.RadioItemIndicator>
                              <Check size={14} />
                            </Menu.RadioItemIndicator>
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                      <Menu.Separator className="my-1 border-t border-border-variant" />
                      <Menu.Item
                        nativeButton
                        render={
                          <MenuItem className="data-highlighted:bg-surface" />
                        }
                        onClick={openHosts}
                      >
                        {m.remote_dialog_title()}
                      </Menu.Item>
                    </>
                  )}
                  {runtime.kind === "ssh" && (
                    <>
                      <div className="border-b border-border-variant px-2 pt-1 pb-2">
                        <div className="text-sm font-medium text-text">
                          {current?.state === "connected"
                            ? m.remote_connected_as({
                              host: ltr(runtime.session.host),
                              user: ltr(runtime.session.user ?? ""),
                            })
                            : ltr(runtime.session.host)}
                        </div>
                        <div className="mt-0.5 text-xs text-subtext">
                          {current && current.state !== "connected" && <>{current.label} · </>}
                          OpenResearch{" "}
                          {ltr(runtime.session.buildId ?? runtime.session.version ?? runtime.version)}
                        </div>
                      </div>
                      <div className="flex items-center rounded-sm hover:bg-surface">
                        <Menu.Item
                          nativeButton
                          render={
                            <MenuItem className="hover:bg-transparent data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default" />
                          }
                          closeOnClick={false}
                          disabled={disconnect.isMutating}
                          onClick={() => disconnect.runWithOptions(workspace, {
                            onSuccessData: (disconnected) => {
                              const { owner } = machines;
                              const runtimeKey = workspace.queries.getRuntimeQuery().queryKey;
                              const sessionsKey = owner.queries.listRemoteSessionsQuery().queryKey;
                              void workspace.queryClient.cancelQueries({ queryKey: runtimeKey });
                              void owner.queryClient.cancelQueries({ queryKey: sessionsKey });
                              workspace.setScopedQueryData(runtimeKey, (current) =>
                                current?.kind === "ssh" ? { ...current, session: disconnected } : current,
                              );
                              owner.setScopedQueryData(sessionsKey, (sessions) =>
                                sessions?.map((session) => session.id === disconnected.id ? disconnected : session),
                              );
                            },
                            onSuccessUi: () => {
                              setOpen(false);
                              if (machines.canSwitch) machines.selectLocal();
                            },
                          })}
                        >
                          {disconnect.isMutating
                            ? m.remote_closing()
                            : m.remote_disconnect()}
                        </Menu.Item>
                        <Tooltip
                          content={m.remote_persistence_explanation()}
                          className="me-2 shrink-0 text-subtext"
                        >
                          <Info size={16} />
                        </Tooltip>
                      </div>
                      <Menu.Item
                        nativeButton
                        render={
                          <MenuItem
                            danger
                            className="data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default"
                          />
                        }
                        closeOnClick={false}
                        disabled={loadingStopPreview}
                        onClick={async () => {
                          setLoadingStopPreview(true);
                          try {
                            const preview = await getRemoteStopPreview();
                            showCustomDialog({
                              component: RemoteStopDialog,
                              props: {
                                host: runtime.session.host,
                                preview,
                                currentClientAttached:
                                  runtime.session.status === "connected",
                              },
                              onConfirm: () => mutStopRemoteHost.runAsHeadlessPromise(workspace, preview, {}),
                              onCancel: "close",
                            });
                            setOpen(false);
                          } catch (error) {
                            showAlert(
                              error instanceof Error
                                ? error.message
                                : String(error),
                              "error",
                            );
                          } finally {
                            setLoadingStopPreview(false);
                          }
                        }}
                      >
                        {m.remote_stop_host()}
                      </Menu.Item>
                    </>
                  )}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
            <Tooltip interactive content={triggerName} className={corner ? "max-w-56" : "min-w-0 max-w-full"}>
              <Menu.Trigger
                data-machine-switcher
                aria-label={triggerName}
                className={`flex min-w-0 items-center gap-1.5 border text-start transition-[background,border-color] duration-120 ease-standard focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2 ${triggerShape} ${triggerTone}`}
              >
                {current?.state === "connecting" ? <Spinner className="size-3.5 border-accent-blue-border border-t-accent-blue-strong" />
                  : current?.state === "disconnected" ? <Unplug size={14} className="shrink-0" aria-hidden="true" />
                    : <RemoteIcon size={14} className={`shrink-0 ${current ? "" : "text-subtext"}`} />}
                <span className="flex min-w-0 flex-col">
                  <span dir="auto" className="truncate text-sm leading-tight">{label}</span>
                  {caption && <span className="truncate text-xs leading-tight">{caption}</span>}
                </span>
                {!corner && <ChevronsUpDown size={14} className="ms-auto shrink-0 opacity-70" aria-hidden="true" />}
              </Menu.Trigger>
            </Tooltip>
          </div>
        </Menu.Root>
      </DirectionProvider>
    </>
  );
}
