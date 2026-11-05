import { useContext, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { useMachines } from "../machineSelection";
import { RemoteHostDialog } from "./RemoteHostDialog";
import { showSshConfigDialog } from "./SshConfigDialog";
import { DirectionProvider } from "@base-ui/react/direction-provider";

import { Check, Info } from "lucide-react";
import { type RuntimeInfo } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Menu } from "@base-ui/react/menu";
import { RemoteIcon } from "./RemoteIcon";

import { RemoteStopDialog } from "./RemoteStopDialog";
import { Button, MenuItem, showAlert, Tooltip } from "./ui";

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

  const {
    disconnectCurrentRemote,
    getRemoteStopPreview,
    stopCurrentRemoteHost,
  } = useWorkspace().api;

  const [disconnecting, setDisconnecting] = useState(false);
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
                ? "workspace-connection-corner fixed bottom-0 start-0 z-50"
                : "relative shrink-0"
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
                              {session.status === "connected"
                                ? m.remote_machine_connected()
                                : session.status === "connecting" ||
                                    session.status === "reconnecting" ||
                                    session.status === "applying"
                                  ? m.remote_machine_connecting()
                                  : m.remote_machine_disconnected()}
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
                          {m.remote_connected_as({
                            host: ltr(runtime.session.host),
                            user: ltr(runtime.session.user ?? ""),
                          })}
                        </div>
                        <div className="mt-0.5 text-xs text-subtext">
                          OpenResearch{" "}
                          {ltr(runtime.session.version ?? runtime.version)}
                        </div>
                      </div>
                      <div className="flex items-center rounded-sm hover:bg-surface">
                        <Menu.Item
                          nativeButton
                          render={
                            <MenuItem className="hover:bg-transparent data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default" />
                          }
                          closeOnClick={false}
                          disabled={disconnecting}
                          onClick={async () => {
                            setDisconnecting(true);
                            try {
                              await disconnectCurrentRemote();
                              setOpen(false);
                              if (machines.canSwitch) machines.selectLocal();
                            } catch (error) {
                              showAlert(
                                error instanceof Error
                                  ? error.message
                                  : String(error),
                                "error",
                              );
                            } finally {
                              setDisconnecting(false);
                            }
                          }}
                        >
                          {disconnecting
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
                              onConfirm: async () => {
                                await stopCurrentRemoteHost(preview);
                              },
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
            {corner ? (
              <Menu.Trigger
                data-machine-switcher
                aria-label={m.remote_machine_switcher({ machine: label })}
                render={<Button variant="default" />}
                className={`h-auto w-auto max-w-48 justify-start rounded-none px-2.5 py-1.5 font-normal ${runtime.kind === "ssh" ? "border-accent-blue bg-accent-blue text-white [&:hover:not(:disabled)]:border-accent-blue [&:hover:not(:disabled)]:bg-accent-blue/90" : "border-border bg-background text-text"}`}
              >
                <RemoteIcon size={14} className="shrink-0" />
                <span className="min-w-0 truncate text-sm leading-tight">
                  {label}
                </span>
              </Menu.Trigger>
            ) : (
              <Tooltip interactive content={label}>
                <Menu.Trigger
                  data-machine-switcher
                  aria-label={m.remote_machine_switcher({ machine: label })}
                  className="flex size-9 shrink-0 items-center justify-center rounded-md text-subtext hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus"
                >
                  <RemoteIcon size={16} aria-hidden="true" />
                </Menu.Trigger>
              </Tooltip>
            )}
          </div>
        </Menu.Root>
      </DirectionProvider>
    </>
  );
}
