import { HotkeysProvider, useHotkey } from "@tanstack/react-hotkeys";
import { useId, type ReactNode } from "react";

function EscapeAction({ onEscape }: { onEscape: () => void }) {
  useHotkey("Escape", onEscape);
  return null;
}

export function EscapeScope({ children, onEscape, enabled = true, exclusive = true }: {
  children?: ReactNode;
  onEscape: () => void;
  enabled?: boolean;
  exclusive?: boolean;
}) {
  return (
    <HotkeysProvider scope={useId()} exclusive={exclusive} enabled={enabled}>
      <EscapeAction onEscape={onEscape} />
      {children}
    </HotkeysProvider>
  );
}
