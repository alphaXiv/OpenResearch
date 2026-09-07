import { createRootRouteWithContext } from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { RuntimeRoot } from "../RemoteRuntime";

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({ component: RuntimeRoot });
