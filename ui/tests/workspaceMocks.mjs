import { createContext } from "react";
export const workspaceDom = { id: (name) => name, portalContainer: undefined };
const WorkspaceDomContext = createContext(workspaceDom);

export function mockWorkspace(bindings) {
  return { ...bindings, api: bindings, queries: bindings, events: bindings, invalidation: bindings, chat: bindings, persistence: bindings, projects: bindings, dialogs: bindings, composers: bindings };
}

export function withWorkspaceMocks(requireModule) {
  const useWorkspace = () => {
    const read = (name) => {
      for (const prefix of ["../", "./"]) {
        try { return requireModule(prefix + name); } catch {}
      }
      return {};
    };
    const merge = (...modules) => new Proxy({}, { get: (_, name) => {
      const owner = modules.find((module) => Object.hasOwn(module, name));
      if (owner) return owner[name];
      return modules.map((module) => module[name]).find((value) => value !== undefined);
    } });
    return {
      ...read("queries/client"),
      api: read("api"),
      queries: merge(...["settings", "projects", "files", "chat"].map((name) => read("queries/" + name))),
      events: read("events"), invalidation: read("queries/invalidation"), chat: read("queries/chatStore"),
      persistence: read("workspacePersistence"), projects: read("useProjectWorkspace"), dialogs: read("dialog"), composers: read("composerStash"),
    };
  };
  return (name) => name === "react" ? (() => {
    const react = requireModule(name);
    return { ...react, useContext: (context) => context === WorkspaceDomContext ? workspaceDom : react.useContext(context) };
  })() : name.endsWith("/machineSelection") ? { useMachines: () => ({ owner: useWorkspace() }) } : name.endsWith("/WorkspaceDom") ? { WorkspaceDomContext } : name === "./workspace" || name === "../workspace" ? { useWorkspace } : requireModule(name);
}
