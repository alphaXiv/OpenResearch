import { createRouter, createMemoryHistory } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";
import { RouteFailure, RouteNotFound, RoutePending } from "./routePages";
import type { Workspace } from "./workspace";

export function createWorkspaceRouter(workspace: Workspace, initialLocation = "/") {
  return createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [initialLocation] }),
    context: { queryClient: workspace.queryClient },
    trailingSlash: "never",
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteFailure,
    defaultNotFoundComponent: RouteNotFound,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createWorkspaceRouter>;
  }
}
