import { useState } from "react";
import { Info } from "lucide-react";
import {
  disconnectCurrentRemote,
  getRemoteStopPreview,
  stopCurrentRemoteHost,
  type RemoteStopPreview,
  type RuntimeInfo,
} from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { usePopover } from "./ModelPicker";
import { RemoteIcon } from "./RemoteIcon";
import { RemoteStopDialog } from "./RemoteStopDialog";
import { Button, IconButton, MenuItem, showAlert, Tooltip } from "./ui";

export function RemoteStatus({
  runtime,
  corner = false,
}: {
  runtime: Extract<RuntimeInfo, { kind: "ssh" }>;
  corner?: boolean;
}) {
  const [disconnecting, setDisconnecting] = useState(false);
  const [loadingStopPreview, setLoadingStopPreview] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [stopPreview, setStopPreview] = useState<RemoteStopPreview | null>(null);
  const menu = usePopover();

  async function confirmStopHost() {
    if (!stopPreview) return;
    setStopping(true);
    try {
      await stopCurrentRemoteHost(stopPreview);
      setStopPreview(null);
    } catch (error) {
      setStopPreview(null);
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setStopping(false);
    }
  }

  return (
    <>
      <div
        className={
          corner
            ? "fixed bottom-0 start-0 z-50"
            : "relative shrink-0 rounded-b-lg border-t border-border bg-background"
        }
        ref={menu.ref}
      >
      {menu.open && (
        <div className="option-menu absolute bottom-[calc(100%_+_6px)] start-2 z-50 min-w-60 rounded-lg border border-border bg-background p-1.5 shadow-menu">
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
            <MenuItem
              className="hover:bg-transparent"
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
            <Tooltip content={m.remote_persistence_explanation()} className="me-2 shrink-0 text-subtext">
              <Info size={15} />
            </Tooltip>
          </div>
          <MenuItem
            danger
            disabled={loadingStopPreview}
            onClick={async () => {
              setLoadingStopPreview(true);
              try {
                setStopPreview(await getRemoteStopPreview());
                menu.setOpen(false);
              } catch (error) {
                showAlert(error instanceof Error ? error.message : String(error), "error");
              } finally {
                setLoadingStopPreview(false);
              }
            }}
          >
            {m.remote_stop_host()}
          </MenuItem>
        </div>
      )}
      {corner ? (
        <Button
          variant="default"
          className="h-auto w-auto max-w-48 justify-start rounded-none border-accent-blue bg-accent-blue px-2.5 py-1.5 font-normal text-white [&:hover:not(:disabled)]:border-accent-blue [&:hover:not(:disabled)]:bg-accent-blue/90"
          aria-haspopup="menu"
          aria-expanded={menu.open}
          onClick={() => menu.setOpen((open) => !open)}
        >
          <RemoteIcon size={14} className="shrink-0" />
          <span className="min-w-0 truncate text-sm leading-tight">
            {m.remote_ssh_host({ host: ltr(runtime.session.host) })}
          </span>
        </Button>
      ) : (
        <div className="flex items-center gap-1.5 py-2 ps-1 pe-2.5">
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
            <span className="-my-0.5 max-w-full self-start truncate rounded-sm bg-accent-blue px-1.5 py-0.5 text-sm leading-tight text-white">
              {m.remote_ssh_host({ host: ltr(runtime.session.host) })}
            </span>
            <span className="truncate text-xs leading-tight text-subtext">
              OpenResearch {ltr(runtime.session.version ?? "…")}
            </span>
          </span>
        </div>
      )}
      </div>
      {stopPreview && (
        <RemoteStopDialog
          host={runtime.session.host}
          preview={stopPreview}
          currentClientAttached={runtime.session.status === "connected"}
          stopping={stopping}
          onClose={() => {
            if (!stopping) setStopPreview(null);
          }}
          onConfirm={() => void confirmStopHost()}
        />
      )}
    </>
  );
}
