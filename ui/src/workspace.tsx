import { createContext, useContext } from "react";
import { createApi } from "./api";
import { createEvents } from "./events";
import { createWorkspaceClient } from "./queries/client";
import { createInvalidation } from "./queries/invalidation";
import { createProjectQueries } from "./queries/projects";
import { createFileQueries } from "./queries/files";
import { createSettingsQueries } from "./queries/settings";
import { createChatQueries } from "./queries/chat";
import { createChatStore } from "./queries/chatStore";
import { createGlobalPersistence } from "./workspacePersistence";
import { createProjectPersistence } from "./useProjectWorkspace";
import { createDialogs } from "./dialog";
import { createComposerState } from "./composerStash";

export function createWorkspace(identity: string, basePath = "") {
  const client = createWorkspaceClient(identity, basePath);
  const invalidation = createInvalidation(client);
  const api = createApi(client, invalidation);
  const events = createEvents(client);
  const queries = {
    ...createProjectQueries(client, api),
    ...createFileQueries(client, api),
    ...createSettingsQueries(client, api, invalidation, events),
    ...createChatQueries(client, api, events),
  };
  const chat = createChatStore(client, queries);
  const persistence = createGlobalPersistence(client, api, queries);
  const projects = createProjectPersistence(client, api, queries);
  const dialogs = createDialogs();
  return {
    ...client,
    api,
    queries,
    events,
    invalidation,
    chat,
    persistence,
    projects,
    dialogs,
    composers: createComposerState(),
    transcriptRowSizes: new Map<string, { size: number; width: number }>(),
    transcriptDisclosures: new Map<string, Map<string, boolean>>(),
  };
}

export type Workspace = ReturnType<typeof createWorkspace>;
export type WorkspaceQueries = ReturnType<typeof createProjectQueries> &
  ReturnType<typeof createFileQueries> &
  ReturnType<typeof createSettingsQueries> &
  ReturnType<typeof createChatQueries>;
export type WorkspaceEvents = ReturnType<typeof createEvents>;
export const WorkspaceContext = createContext<Workspace | null>(null);

export function useWorkspace() {
  const workspace = useContext(WorkspaceContext);
  if (!workspace) throw new Error("Workspace is not connected");
  return workspace;
}
