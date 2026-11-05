import { createContext } from "react";

export const WorkspaceDomContext = createContext<{
  portalContainer: HTMLElement | undefined;
  id: (name: string) => string;
}>({ portalContainer: undefined, id: (name) => name });
