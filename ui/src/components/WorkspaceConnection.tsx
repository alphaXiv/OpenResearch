import { useState } from "react";
import type { RuntimeInfo } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { RemoteHostDialog } from "./RemoteHostDialog";
import { RemoteIcon } from "./RemoteIcon";
import { RemoteStatus } from "./RemoteStatus";
import { SshConfigDialog } from "./SshConfigDialog";
import { Button, IconButton } from "./ui";

export function WorkspaceConnection({
  runtime,
  corner = false,
}: {
  runtime: RuntimeInfo;
  corner?: boolean;
}) {
  const [dialog, setDialog] = useState<"hosts" | "config" | null>(null);

  if (runtime.kind === "ssh") return <RemoteStatus runtime={runtime} corner={corner} />;

  return (
    <>
      {corner ? (
        <div className="fixed bottom-0 start-0 z-50">
          <Button
            className="h-auto max-w-48 justify-start rounded-none border-b-0 border-s-0 px-2.5 py-1.5 font-normal"
            title={m.remote_dialog_title()}
            aria-haspopup="dialog"
            onClick={() => setDialog("hosts")}
          >
            <RemoteIcon size={14} className="shrink-0" />
            <span className="min-w-0 truncate text-sm leading-tight">{m.projects_local()}</span>
          </Button>
        </div>
      ) : (
        <div className="relative shrink-0 border-t border-border">
          <div className="flex items-center gap-1.5 py-2 ps-1 pe-2.5">
            <IconButton
              size="small"
              aria-label={m.remote_dialog_title()}
              aria-haspopup="dialog"
              onClick={() => setDialog("hosts")}
            >
              <RemoteIcon size={14} className="shrink-0" />
            </IconButton>
            <span className="flex min-w-0 flex-col gap-1 text-start text-text">
              <span className="truncate text-sm leading-tight">{m.projects_local()}</span>
              <span className="truncate text-xs leading-tight text-subtext">OpenResearch {ltr(runtime.version)}</span>
            </span>
          </div>
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
