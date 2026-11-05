import { createContext, useContext } from "react";
import type { RemoteSessionInfo } from "./api";
import type { Workspace } from "./workspace";

export interface MachineSelection {
  owner: Workspace;
  activeId: string;
  sessions: RemoteSessionInfo[];
  canSwitch: boolean;
  selectLocal: () => void;
  selectRemote: (session: RemoteSessionInfo) => void;
}

export const MachineSelectionContext = createContext<MachineSelection | null>(
  null,
);
export function useMachines() {
  const machines = useContext(MachineSelectionContext);
  if (!machines) throw new Error("Machine switcher is unavailable");
  return machines;
}
