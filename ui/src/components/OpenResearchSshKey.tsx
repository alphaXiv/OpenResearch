import { useMutate } from "@clo/react-mutation";
import { KeyRound } from "lucide-react";
import type { OpenResearchSettings } from "../api";
import { m } from "../paraglide/messages.js";
import { mutAddOpenResearchSshKey } from "../queries/settings";
import { useWorkspace } from "../workspace";
import { Button, Spinner, showAlert } from "./ui";

export function OpenResearchSshKey({ sshKey }: { sshKey: NonNullable<OpenResearchSettings["sshKeyToAdd"]> }) {
  const workspace = useWorkspace();
  const add = useMutate(mutAddOpenResearchSshKey);
  const failure = add.error;

  return (
    <div className="mt-4 rounded-lg border border-border p-4">
      <code dir="ltr" className="block break-all font-mono text-sm text-text">{sshKey.path}</code>
      {sshKey.fingerprint
        ? <code dir="ltr" className="mt-1 block break-all font-mono text-xs text-subtext">{sshKey.fingerprint}</code>
        : <span className="mt-1 block text-sm text-subtext">{m.settings_ssh_key_new()}</span>}
      <p dir="auto" className="mt-3 mb-0 text-sm text-subtext">{m.settings_ssh_key_upload_note()}</p>
      {failure != null && <p role="alert" dir="auto" className="mt-3 mb-0 whitespace-pre-wrap break-words text-sm text-accent-red">{failure instanceof Error ? failure.message : String(failure)}</p>}
      <div className="mt-4 flex justify-end">
        <Button variant="primary" disabled={add.isMutating} onClick={() => add.runWithOptions(workspace, { onSuccessUi: () => showAlert(m.settings_ssh_key_added(), "success"), onError: () => {} })}>
          {add.isMutating ? <Spinner /> : <KeyRound size={14} />} {add.isMutating ? m.settings_ssh_key_adding() : add.isError ? m.app_retry() : m.settings_ssh_key_add()}
        </Button>
      </div>
    </div>
  );
}
