import { createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";
import { RouteFailure, RouteNotFound, RoutePending } from "./routePages";

export const router = createRouter({
  routeTree,
  trailingSlash: "never",
  defaultPendingComponent: RoutePending,
  defaultErrorComponent: RouteFailure,
  defaultNotFoundComponent: RouteNotFound,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
