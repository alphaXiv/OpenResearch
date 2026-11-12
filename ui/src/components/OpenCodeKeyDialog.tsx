import { useRef, useState } from "react";
import type { OpenCodeProvider } from "../api";
import type { DialogContentProps } from "../dialog";
import { useWorkspace } from "../workspace";
import { m } from "../paraglide/messages.js";
import { KeyField, providerTile, TileMark } from "./AddAgentDialog";
import { AgentIconTile, BRAND_ICON } from "./AgentIcon";
import { Button, DialogContent, DialogTitle, Spinner } from "./ui";

/** Replaces the key OpenResearch keeps for a provider. */
export function OpenCodeKeyDialog({ provider, confirm, cancel, isDisabled, isPending, errorMessage }: DialogContentProps<() => Promise<void>> & {
  provider: OpenCodeProvider;
}) {
  const { replaceOpenCodeProviderKey } = useWorkspace().api;
  const { refreshHarness } = useWorkspace().queries;

  const [key, setKey] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const ready = key.trim().length > 0 && !isDisabled;
  const { tile, key: method } = providerTile(provider);

  return (
    <DialogContent
      initialFocus={inputRef}
      className="w-140 p-5 text-text"
      render={<form onSubmit={(event) => {
        event.preventDefault();
        // The row shows the re-check; the dialog only waits on the save.
        if (ready) confirm(async () => { await replaceOpenCodeProviderKey(provider.id, key.trim()); void refreshHarness("opencode").catch(() => {}); });
      }} />}
    >
      <div className="flex items-center gap-3">
        <AgentIconTile icon={provider.icon ?? BRAND_ICON} brand={(size) => <TileMark tile={tile} size={size} />} />
        <DialogTitle className="m-0 text-xl font-medium">{m.provider_key_replace_title({ name: provider.name })}</DialogTitle>
      </div>
      <div className="mt-4">
        <KeyField method={method} name={tile.name} value={key} disabled={isDisabled} onChange={setKey} inputRef={inputRef} />
      </div>
      {errorMessage && <p role="alert" className="mt-3 text-sm text-accent-red">{errorMessage}</p>}
      <div className="mt-5 flex justify-end gap-2">
        <Button type="button" onClick={cancel} disabled={isPending}>{m.settings_page_cancel()}</Button>
        <Button type="submit" variant="primary" disabled={!ready}>
          {isPending && <Spinner />}{m.provider_key_replace()}
        </Button>
      </div>
    </DialogContent>
  );
}
