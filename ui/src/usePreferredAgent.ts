import { type SetStateAction, useCallback, useRef } from "react";

import { type AgentSelection, type UiState } from "./api";

/** Persists the preferred-agent selection with an optimistic update, a
 * serialized write queue (so out-of-order saves can't clobber a newer
 * selection), and rollback to the last-saved value on failure. */
export function usePreferredAgent(
  setUiState: (value: SetStateAction<UiState | null>) => void,
  saveUiState: (patch: { preferredAgent: AgentSelection }) => Promise<UiState>,
) {
  const persistedPreferredAgent = useRef<AgentSelection | null>(null);
  const preferredAgentWrite = useRef<Promise<void>>(Promise.resolve());
  const preferredAgentSaveSeq = useRef(0);
  const persistPreferredAgent = useCallback((selection: AgentSelection) => {
    const saveSeq = ++preferredAgentSaveSeq.current;
    setUiState((current) => current && { ...current, preferredAgent: selection });
    const write = preferredAgentWrite.current
      .then(() => saveUiState({ preferredAgent: selection }))
      .then((saved) => {
        persistedPreferredAgent.current = saved.preferredAgent;
        if (saveSeq === preferredAgentSaveSeq.current) {
          setUiState((current) => current && { ...current, preferredAgent: saved.preferredAgent });
        }
      })
      .catch((error: unknown) => {
        if (saveSeq === preferredAgentSaveSeq.current) {
          setUiState((current) =>
            current && { ...current, preferredAgent: persistedPreferredAgent.current },
          );
        }
        throw error;
      });
    preferredAgentWrite.current = write.catch(() => {});
    return write;
  }, [setUiState, saveUiState]);

  return { persistedPreferredAgent, persistPreferredAgent };
}
