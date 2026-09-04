import { useState } from "react";
import {
  disconnectCurrentRemote,
  getRemoteStopPreview,
  stopCurrentRemoteHost,
  type RuntimeInfo,
} from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { usePopover } from "./ModelPicker";
import { RemoteIcon } from "./RemoteIcon";
import { Button, IconButton, MenuItem, showAlert } from "./ui";

export function RemoteStatus({
  runtime,
  corner = false,
}: {
  runtime: Extract<RuntimeInfo, { kind: "ssh" }>;
  corner?: boolean;
}) {
  const [disconnecting, setDisconnecting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const menu = usePopover();

  return (
    <div
      className={
        corner
          ? "fixed bottom-0 start-0 z-50"
          : "relative shrink-0 rounded-b-lg border-t border-border bg-background"
      }
      ref={menu.ref}
    >
      {menu.open && (
        <div className="option-menu absolute bottom-[calc(100%_+_6px)] start-2 z-50 min-w-60 overflow-hidden rounded-lg border border-border bg-background p-1.5 shadow-menu">
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
            <p className="mt-2 mb-0 max-w-72 text-xs leading-relaxed text-subtext">
              {m.remote_persistence_explanation()}
            </p>
          </div>
          <MenuItem
            disabled={disconnecting}
            onClick={async () => {
              setDisconnecting(true);
              try {
                await disconnectCurrentRemote();
                menu.setOpen(false);
              } catch (error) {
                showAlert(error instanceof Error ? error.message : String(error), "error");
              } finally {
                setDisconnecting(false);
              }
            }}
          >
            {disconnecting ? m.remote_closing() : m.remote_disconnect()}
          </MenuItem>
          <MenuItem
            danger
            disabled={stopping}
            onClick={async () => {
              setStopping(true);
              try {
                const preview = await getRemoteStopPreview();
                if (!window.confirm(m.remote_stop_confirm({
                  turns: String(preview.activeTurnCount),
                  clients: String(preview.attachmentCount),
                  runs: String(preview.activeRunCount),
                  queued: String(preview.queuedMessageCount),
                  approvals: String(preview.pendingPermissionCount),
                }))) return;
                await stopCurrentRemoteHost(preview);
                menu.setOpen(false);
              } catch (error) {
                showAlert(error instanceof Error ? error.message : String(error), "error");
              } finally {
                setStopping(false);
              }
            }}
          >
            {stopping ? m.remote_stopping_host() : m.remote_stop_host()}
          </MenuItem>
        </div>
      )}
      {corner ? (
        <Button
          variant="default"
          className="h-auto w-auto justify-start rounded-none border-accent-blue bg-accent-blue px-2.5 py-1.5 font-normal text-white [&:hover:not(:disabled)]:border-accent-blue [&:hover:not(:disabled)]:bg-accent-blue/90"
          aria-haspopup="menu"
          aria-expanded={menu.open}
          onClick={() => menu.setOpen((open) => !open)}
        >
          <RemoteIcon size={14} className="shrink-0" />
          <span className="truncate text-sm leading-tight">
            {m.remote_ssh_host({ host: ltr(runtime.session.host) })}
          </span>
        </Button>
      ) : (
        <div className="flex items-center gap-1.5 px-1 py-2">
          <IconButton
            size="small"
            aria-label={m.remote_ssh_host({ host: ltr(runtime.session.host) })}
            aria-haspopup="menu"
            aria-expanded={menu.open}
            onClick={() => menu.setOpen((open) => !open)}
          >
            <RemoteIcon size={14} className="shrink-0" />
          </IconButton>
          <span className="flex min-w-0 flex-col gap-1 text-start text-text">
            <span className="-my-0.5 self-start truncate rounded-sm bg-accent-blue px-1.5 py-0.5 text-sm leading-tight text-white">
              {m.remote_ssh_host({ host: ltr(runtime.session.host) })}
            </span>
            <span className="truncate text-xs leading-tight text-subtext">
              OpenResearch {ltr(runtime.session.version ?? "…")}
            </span>
          </span>
        </div>
      )}
    </div>
  );
}
