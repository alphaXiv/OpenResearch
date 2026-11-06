import { useEffect, useRef, useState, type RefObject } from "react";
import type { ComposerEditorHandle } from "./ComposerEditor";

/**
 * Focuses the composer once per new `nonce`, and once when a new chat requested
 * through the returned function opens in that project. The composer is inert
 * until `ready`, so a request stays pending until it can take focus.
 */
export function useComposerFocus(
  composerRef: RefObject<ComposerEditorHandle | null>,
  nonce: number,
  ready: boolean,
  projectId: string,
  activeId: string | null,
): (projectId: string) => void {
  const [newChatProject, setNewChatProject] = useState<string | null>(null);
  const focusedNonce = useRef(0);
  useEffect(() => {
    const newChatPending = newChatProject === projectId && activeId === null;
    const composerPending = nonce !== 0 && nonce !== focusedNonce.current;
    if ((!newChatPending && !composerPending) || !ready) return;
    // The welcome dialog restores its previous focus on unmount; run after that.
    const frame = window.requestAnimationFrame(() => {
      if (document.activeElement?.closest("[role=search]")) {
        focusedNonce.current = nonce;
        if (newChatPending) setNewChatProject(null);
        return;
      }
      const composer = composerRef.current;
      if (!composer?.dom) return;
      composer.focus();
      if (!composer.dom.contains(document.activeElement)) return;
      focusedNonce.current = nonce;
      if (newChatPending) setNewChatProject(null);
      composer.focus("end");
      composer.dom.scrollIntoView({ block: "nearest" });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [composerRef, nonce, newChatProject, projectId, activeId, ready]);
  return setNewChatProject;
}
