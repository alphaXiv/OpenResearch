import { useMutation } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useId } from "react";
import type { HarnessId } from "../api";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { Button } from "./ui";
import { cn } from "./ui/cn";

export function useAgentRefresh(harness?: HarnessId, step?: string) {
  const { queries: { refreshHarnesses, refreshHarness }, queryClient, workspaceKey } = useWorkspace();
  const mutationKey = workspaceKey("refreshAgents", useId(), step);
  const refresh = useMutation({ mutationKey, mutationFn: () => harness ? refreshHarness(harness) : refreshHarnesses(true, true) });
  const start = () => {
    if (queryClient.isMutating({ mutationKey }) === 0) refresh.mutate();
  };
  return { ...refresh, start };
}

export function RefreshAgents({ refresh, disabled, label = m.settings_page_refresh() }: {
  refresh: ReturnType<typeof useAgentRefresh>;
  disabled?: boolean;
  label?: string | null;
}) {
  if (label === null && refresh.isIdle) return null;

  return (
    <div className="flex flex-wrap items-center gap-2">
      {label !== null && <Button type="button" onClick={refresh.start} disabled={disabled || refresh.isPending} aria-busy={refresh.isPending}>
        <RefreshCw size={14} aria-hidden="true" className={cn("text-subtext", refresh.isPending && "motion-safe:animate-spin")} />{label}
      </Button>}
      <span role="status" className="text-sm text-subtext">
        {refresh.isPending ? m.common_checking() : refresh.isSuccess ? m.harness_refresh_complete() : null}
      </span>
      {refresh.error && <span role="alert" className="basis-full text-sm text-accent-red wrap-anywhere">{refresh.error.message}</span>}
    </div>
  );
}
