import { DirectionProvider } from "@base-ui/react/direction-provider";
import { useState } from "react";
import { Info } from "lucide-react";
import {
  disconnectCurrentRemote,
  getRemoteStopPreview,
  stopCurrentRemoteHost,
  type RuntimeInfo,
} from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Menu } from "@base-ui/react/menu";
import { RemoteIcon } from "./RemoteIcon";
import { showCustomDialog } from "../dialog";
import { RemoteStopDialog } from "./RemoteStopDialog";
import { Button, MenuItem, showAlert, Tooltip } from "./ui";

export function RemoteStatus({
  runtime,
  corner = false,
}: {
  runtime: Extract<RuntimeInfo, { kind: "ssh" }>;
  corner?: boolean;
}) {
  const [disconnecting, setDisconnecting] = useState(false);
  const [loadingStopPreview, setLoadingStopPreview] = useState(false);
  const [open, setOpen] = useState(false);

  return (
    <>
      <DirectionProvider
        direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}
      >
        <Menu.Root open={open} onOpenChange={setOpen} modal={false}>
          <div className={corner ? "fixed bottom-0 start-0 z-50" : "relative shrink-0"}>
            <Menu.Portal>
              <Menu.Positioner side="top" align="start" sideOffset={6} collisionPadding={8} className="z-50">
                <Menu.Popup className="outline-none max-h-(--available-height) overflow-y-auto option-menu min-w-60 rounded-lg border border-border bg-background p-1.5 shadow-menu">
                  <div className="border-b border-border-variant px-2 pt-1 pb-2">
                    <div className="text-sm font-medium text-text">
                      {m.remote_connected_as({
                        host: ltr(runtime.session.host),
                        user: ltr(runtime.session.user ?? ""),
                      })}
                    </div>
                    <div className="mt-0.5 text-xs text-subtext">
                      OpenResearch {ltr(runtime.session.version ?? "…")}
                    </div>
                  </div>
                  <div className="flex items-center rounded-sm hover:bg-surface">
                    <Menu.Item
                      nativeButton
                      render={<MenuItem className="hover:bg-transparent data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default" />}
                      closeOnClick={false}
                      disabled={disconnecting}
                      onClick={async () => {
                        setDisconnecting(true);
                        try {
                          await disconnectCurrentRemote();
                          setOpen(false);
                        } catch (error) {
                          showAlert(error instanceof Error ? error.message : String(error), "error");
                        } finally {
                          setDisconnecting(false);
                        }
                      }}
                    >
                      {disconnecting ? m.remote_closing() : m.remote_disconnect()}
                    </Menu.Item>
                    <Tooltip content={m.remote_persistence_explanation()} className="me-2 shrink-0 text-subtext">
                      <Info size={16} />
                    </Tooltip>
                  </div>
                  <Menu.Item
                    nativeButton
                    render={<MenuItem danger className="data-highlighted:bg-surface data-disabled:opacity-45 data-disabled:cursor-default" />}
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
                            currentClientAttached: runtime.session.status === "connected",
                          },
                          onConfirm: async () => { await stopCurrentRemoteHost(preview); },
                          onCancel: "close",
                        });
                        setOpen(false);
                      } catch (error) {
                        showAlert(error instanceof Error ? error.message : String(error), "error");
                      } finally {
                        setLoadingStopPreview(false);
                      }
                    }}
                  >
                    {m.remote_stop_host()}
                  </Menu.Item>
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
            {corner ? (
              <Menu.Trigger
                render={<Button variant="default" />}
                className="h-auto w-auto max-w-48 justify-start rounded-none border-accent-blue bg-accent-blue px-2.5 py-1.5 font-normal text-white [&:hover:not(:disabled)]:border-accent-blue [&:hover:not(:disabled)]:bg-accent-blue/90"
              >
                <RemoteIcon size={14} className="shrink-0" />
                <span className="min-w-0 truncate text-sm leading-tight">
                  {m.remote_ssh_host({ host: ltr(runtime.session.host) })}
                </span>
              </Menu.Trigger>
            ) : (
              <Tooltip interactive content={m.account_local_to_app()}><Menu.Trigger className="relative flex size-9 shrink-0 items-center justify-center rounded-md text-subtext hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus" aria-label={`${m.remote_ssh_host({ host: ltr(runtime.session.host) })} · ${m.account_local_to_app()}`}>
                <RemoteIcon size={16} /><span aria-hidden="true" className="absolute end-1 bottom-1 size-2 rounded-full bg-accent-blue" />
              </Menu.Trigger></Tooltip>
            )}
          </div>
        </Menu.Root>
      </DirectionProvider>
    </>
  );
}
