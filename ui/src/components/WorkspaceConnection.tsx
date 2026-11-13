import { useContext, useRef, useState, useSyncExternalStore } from "react";
import { useMutate } from "@clo/react-mutation";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { useMachines } from "../machineSelection";
import { showRemoteHostDialog } from "./RemoteHostDialog";
import { DirectionProvider } from "@base-ui/react/direction-provider";

import { Check, ChevronsUpDown, CloudOff, Info, Laptop, Plus, Power, Server, Unplug } from "lucide-react";
import { type RemoteSessionStatus, type RuntimeInfo } from "../api";
import { autoDir, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutDisconnectRemote, mutStopRemoteHost } from "../queries/settings";
import { Menu } from "@base-ui/react/menu";
import { RemoteIcon } from "./RemoteIcon";

import { RemoteStopDialog } from "./RemoteStopDialog";
import { MenuItem, showAlert, Spinner, StatusIndicator, Tooltip } from "./ui";

const ITEM = "justify-start data-highlighted:bg-surface";
const ICON = "shrink-0 text-subtext";

function connection(status: RemoteSessionStatus) {
  if (status === "connected") return { state: "connected", label: m.remote_machine_connected() } as const;
  if (status === "connecting" || status === "reconnecting" || status === "applying") return { state: "connecting", label: m.remote_machine_connecting() } as const;
  return { state: "disconnected", label: m.remote_machine_disconnected() } as const;
}

/** Holds the check's width on every row so statuses line up. */
function Selected() {
  return (
    <span className="flex size-3.5 shrink-0">
      <Menu.RadioItemIndicator>
        <Check size={14} />
      </Menu.RadioItemIndicator>
    </span>
  );
}

function SessionStatus({ status }: { status: RemoteSessionStatus }) {
  const { state, label } = connection(status);
  return (
    <StatusIndicator
      tone={state === "connected" ? "success" : state === "connecting" ? "info" : "neutral"}
      live={state === "connecting"}
      stale={state === "disconnected"}
      className="text-xs font-normal text-subtext"
    >
      {label}
    </StatusIndicator>
  );
}

/** The workspace's machine as the status strip shows it: `current` is null for this computer, `live` whether its event stream is up. */
export function useMachineState(runtime: RuntimeInfo) {
  const machines = useMachines();
  const { onConnectionChange, isConnected } = useWorkspace().events;
  const live = useSyncExternalStore(onConnectionChange, isConnected, isConnected);
  const activeSession = machines.sessions.find((session) => session.id === machines.activeId);
  const current = runtime.kind === "ssh" ? connection(activeSession?.status ?? runtime.session.status) : null;
  return { current, live };
}

export function WorkspaceConnection({
  runtime,
  corner = false,
}: {
  runtime: RuntimeInfo;
  /** On error screens, fixed in the window's corner; otherwise the status strip's segment, which grows into its free space. */
  corner?: boolean;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const machines = useMachines();
  const workspace = useWorkspace();
  const { showCustomDialog } = workspace.dialogs;
  const openHosts = () => showRemoteHostDialog(workspace.dialogs);
  const label = runtime.kind === "local" ? m.projects_local() : runtime.session.host;
  const { current, live } = useMachineState(runtime);
  const machine = runtime.kind === "ssh" ? autoDir(label) : label;
  // In the strip, a machine that can't be reached is the offline notice: the cloud and its name in amber.
  const offline = !corner && (current?.state === "disconnected" || !live);
  const status = offline ? m.remote_machine_disconnected() : current?.state === "connected" ? null : current?.label;
  const caption = offline ? null : status;
  const triggerName = status
    ? m.remote_machine_switcher_status({ machine, status })
    : m.remote_machine_switcher({ machine });
  const triggerShape = corner ? "rounded-none px-2.5 py-1.5" : "h-7 flex-1 rounded-md px-2";
  const triggerTone = corner
    ? current
      ? "border-accent-blue-border bg-accent-blue-subtle text-accent-blue-strong hover:border-accent-blue"
      : "border-border bg-background text-text hover:bg-surface"
    : offline ? "border-transparent text-accent-amber hover:bg-surface data-popup-open:bg-surface"
      : current ? "border-transparent bg-accent-blue-subtle text-accent-blue-strong"
        : "border-transparent text-text hover:bg-surface data-popup-open:bg-surface";

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
                : "relative flex min-w-0 flex-1"
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
                          render={<MenuItem className={ITEM} />}
                          onClick={() => {
                            switching.current = machines.activeId !== "local";
                            setOpen(false);
                            machines.selectLocal();
                          }}
                        >
                          <Laptop size={14} className={ICON} aria-hidden="true" />
                          <span className="min-w-0 flex-1 truncate">{m.projects_local()}</span>
                          <Selected />
                        </Menu.RadioItem>
                        {machines.sessions.map((session) => (
                          <Menu.RadioItem
                            nativeButton
                            key={session.id}
                            value={session.id}
                            closeOnClick
                            render={<MenuItem className={ITEM} />}
                            onClick={() => {
                              switching.current =
                                machines.activeId !== session.id;
                              setOpen(false);
                              machines.selectRemote(session);
                            }}
                          >
                            <Server size={14} className={ICON} aria-hidden="true" />
                            <span className="min-w-0 flex-1 truncate">
                              {session.host}
                            </span>
                            <SessionStatus status={session.status} />
                            <Selected />
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                      <Menu.Item
                        nativeButton
                        render={<MenuItem className={ITEM} />}
                        onClick={openHosts}
                      >
                        <Plus size={14} className={ICON} aria-hidden="true" />
                        <span className="min-w-0 flex-1 truncate">{m.remote_dialog_title()}</span>
                      </Menu.Item>
                    </>
                  )}
                  {runtime.kind === "ssh" && (
                    <Menu.Group>
                      {machines.canSwitch && (
                        <Menu.Separator className="my-1 border-t border-border-variant" />
                      )}
                      <Menu.GroupLabel className="px-2 pt-1.5 pb-1 text-xs text-muted">
                        <span className="block truncate" dir="ltr">
                          {runtime.session.user
                            ? `${runtime.session.user}@${runtime.session.host}`
                            : runtime.session.host}
                        </span>
                        <span className="block">
                          {current && current.state !== "connected" && <>{current.label} · </>}
                          OpenResearch{" "}
                          {ltr(runtime.session.buildId ?? runtime.session.version ?? runtime.version)}
                        </span>
                      </Menu.GroupLabel>
                      <div className="flex items-center rounded-sm hover:bg-surface">
                        <Menu.Item
                          nativeButton
                          render={
                            <MenuItem className="justify-start hover:bg-transparent data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default" />
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
                          <Unplug size={14} className={ICON} aria-hidden="true" />
                          {disconnect.isMutating
                            ? m.remote_closing()
                            : m.remote_disconnect()}
                        </Menu.Item>
                        <Tooltip
                          content={m.remote_persistence_explanation()}
                          className="me-2 shrink-0 text-subtext"
                        >
                          <Info size={14} />
                        </Tooltip>
                      </div>
                      <Menu.Item
                        nativeButton
                        render={
                          <MenuItem
                            danger
                            className="justify-start data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default"
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
                        <Power size={14} className="shrink-0" aria-hidden="true" />
                        {m.remote_stop_host()}
                      </Menu.Item>
                    </Menu.Group>
                  )}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
            <Tooltip interactive content={triggerName} className={corner ? "max-w-56" : "min-w-0 flex-1"}>
              <Menu.Trigger
                data-machine-switcher
                aria-label={triggerName}
                className={`flex min-w-0 items-center gap-1.5 border text-start transition-[background,border-color] duration-120 ease-standard focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2 ${triggerShape} ${triggerTone}`}
              >
                {offline ? <CloudOff size={14} className="shrink-0" aria-hidden="true" />
                  : current?.state === "connecting" ? <Spinner className="size-3.5 border-accent-blue-border border-t-accent-blue-strong" />
                  : current?.state === "disconnected" ? <Unplug size={14} className="shrink-0" aria-hidden="true" />
                    : <RemoteIcon size={14} className={`shrink-0 ${current ? "" : "text-subtext"}`} />}
                {corner ? <span className="flex min-w-0 flex-col">
                  <span dir="auto" className="truncate text-sm leading-tight">{label}</span>
                  {caption && <span className="truncate text-xs leading-tight">{caption}</span>}
                </span> : <>
                  <span data-machine-name className="flex min-w-0 flex-1 items-baseline gap-1">
                    <span dir="auto" className="min-w-0 truncate text-sm">{label}</span>
                    {caption && <span className="min-w-0 shrink-[100] truncate text-xs opacity-80 @max-[280px]:hidden">{caption}</span>}
                  </span>
                  <ChevronsUpDown size={12} className="shrink-0 opacity-60 @max-[250px]:hidden" aria-hidden="true" />
                </>}
              </Menu.Trigger>
            </Tooltip>
          </div>
        </Menu.Root>
      </DirectionProvider>
    </>
  );
}
