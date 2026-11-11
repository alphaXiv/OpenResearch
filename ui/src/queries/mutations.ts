import { MutationClient } from "@clo/react-mutation";
import { queryClientOptimisticHelpers } from "@clo/react-mutation/tanstack-query";
import type { QueryClient } from "@tanstack/react-query";

import { showAlert } from "../components/ui";

type Helpers = ReturnType<ReturnType<typeof queryClientOptimisticHelpers>>;

/** Mutations take the workspace they write to as their first argument, so one definition serves every machine. */
export const mutations = new MutationClient({
  context: {},
  getOptimisticHelpers: (events) => {
    const byClient = new Map<QueryClient, Helpers>();
    return (queryClient: QueryClient) => {
      const helpers = byClient.get(queryClient) ?? queryClientOptimisticHelpers(queryClient)(events);
      byClient.set(queryClient, helpers);
      return helpers;
    };
  },
  // `message` is the library's English "Could not …"; the API's own message is written for the user.
  reportError: (_message, error) => showAlert(error instanceof Error ? error.message : String(error), "error"),
});
