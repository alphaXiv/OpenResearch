import { useState } from "react";
import type { RuntimeInfo } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { RemoteHostDialog } from "./RemoteHostDialog";
import { RemoteIcon } from "./RemoteIcon";
import { RemoteStatus } from "./RemoteStatus";
import { SshConfigDialog } from "./SshConfigDialog";
import { BrandMark } from "./Wordmark";
import { useUpdateStatus } from "./UpdateBanner";
import { Button, Tooltip } from "./ui";

export function WorkspaceConnection({
  runtime,
  corner = false,
}: {
  runtime: RuntimeInfo;
  corner?: boolean;
}) {
  const [dialog, setDialog] = useState<"hosts" | "config" | null>(null);
  const { status } = useUpdateStatus(runtime.kind === "local");
  const canary = status?.current.startsWith("canary-") ? status.current.slice("canary-".length) : null;

  if (runtime.kind === "ssh") return <RemoteStatus runtime={runtime} corner={corner} />;

  return (
    <>
      {corner ? (
        <div className="fixed bottom-0 start-0 z-50">
          <Tooltip interactive content={m.remote_dialog_title()} className="rounded-none">
            <Button
              className="h-auto max-w-48 justify-start rounded-none border-b-0 border-s-0 px-2.5 py-1.5 font-normal"
              aria-haspopup="dialog"
              onClick={() => setDialog("hosts")}
            >
              <RemoteIcon size={14} className="shrink-0" />
              <span className="min-w-0 truncate text-sm leading-tight">{m.projects_local()}</span>
            </Button>
          </Tooltip>
        </div>
      ) : (
        <div className="relative shrink-0 border-t border-border">
          <button
            type="button"
            className="flex w-full items-center gap-1.5 py-2 ps-1 pe-2.5 hover:bg-surface active:bg-highlight focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:-outline-offset-2"
            aria-label={`${m.projects_local()} · OpenResearch ${canary ? `${m.settings_update_channel_canary()} ${canary}` : runtime.version} · ${m.remote_dialog_title()}`}
            aria-haspopup="dialog"
            onClick={() => setDialog("hosts")}
          >
            <span className="flex size-7 shrink-0 items-center justify-center text-subtext">
              <RemoteIcon size={14} className="shrink-0" />
            </span>
            <span className="flex min-w-0 flex-col gap-1 text-start text-text">
              <span className="truncate text-sm leading-tight">{m.projects_local()}</span>
              <span className="flex min-w-0 items-center gap-1 text-xs leading-tight text-subtext">
                <span className="size-3 shrink-0"><BrandMark /></span>
                <span className="truncate">OpenResearch {canary ? <span className="rounded-sm bg-panel px-1">{m.settings_update_channel_canary()} {ltr(canary)}</span> : ltr(runtime.version)}</span>
              </span>
            </span>
          </button>
        </div>
      )}
      {dialog === "hosts" && (
        <RemoteHostDialog
          onClose={() => setDialog(null)}
          onConfigureSsh={() => setDialog("config")}
        />
      )}
      {dialog === "config" && <SshConfigDialog onClose={() => setDialog("hosts")} />}
    </>
  );
}
