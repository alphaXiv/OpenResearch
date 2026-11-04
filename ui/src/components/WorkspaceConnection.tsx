import { showCustomDialog } from "../dialog";
import type { RuntimeInfo } from "../api";
import { m } from "../paraglide/messages.js";
import { RemoteHostDialog } from "./RemoteHostDialog";
import { RemoteIcon } from "./RemoteIcon";
import { RemoteStatus } from "./RemoteStatus";
import { showSshConfigDialog } from "./SshConfigDialog";
import { Button, Tooltip } from "./ui";

export function WorkspaceConnection({
  runtime,
  corner = false,
}: {
  runtime: RuntimeInfo;
  corner?: boolean;
}) {
  const openHosts = () => showCustomDialog({
    component: RemoteHostDialog,
    props: {
      onConfigureSsh: () => showSshConfigDialog(openHosts),
    },
    onConfirm: (openRemote) => openRemote(),
    onCancel: "close",
  });

  if (runtime.kind === "ssh") return <RemoteStatus runtime={runtime} corner={corner} />;

  return (
    <>
      {corner ? (
        <div className="fixed bottom-0 start-0 z-50">
          <Tooltip interactive content={m.remote_dialog_title()} className="rounded-none">
            <Button
              className="h-auto max-w-48 justify-start rounded-none border-b-0 border-s-0 px-2.5 py-1.5 font-normal"
              aria-haspopup="dialog"
              onClick={openHosts}
            >
              <RemoteIcon size={14} className="shrink-0" />
              <span className="min-w-0 truncate text-sm leading-tight">{m.projects_local()}</span>
            </Button>
          </Tooltip>
        </div>
      ) : (
        <Tooltip interactive content={`${m.projects_local()} · ${m.remote_dialog_title()}`}>
          <button type="button" className="flex size-9 shrink-0 items-center justify-center rounded-md text-subtext hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus" aria-label={`${m.projects_local()} · ${m.remote_dialog_title()}`} aria-haspopup="dialog" onClick={openHosts}><RemoteIcon size={16} /></button>
        </Tooltip>
      )}
    </>
  );
}
