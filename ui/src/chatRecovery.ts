import type { TurnOptions } from "./api";

export interface RetryMetadata {
  retryOwner?: "native" | "orx";
  attempt?: number;
  maximum?: number | null;
  nextRetryAt?: number | null;
}

export function retryStatusLabel(input: RetryMetadata, now: number): string {
  const seconds = typeof input.nextRetryAt === "number"
    ? Math.max(0, Math.ceil((input.nextRetryAt - now) / 1000))
    : null;
  if (input.retryOwner === "native" && input.maximum == null && seconds == null) {
    return "CLI is retrying…";
  }
  return [
    "Retrying",
    typeof input.attempt === "number"
      ? typeof input.maximum === "number"
        ? `attempt ${input.attempt}/${input.maximum}`
        : `attempt ${input.attempt}`
      : null,
    seconds == null ? null : `next attempt in ${seconds}s`,
  ].filter(Boolean).join(" · ");
}

export function queuedRetryLabel(nextRetryAt: number | null | undefined, now: number): string {
  if (typeof nextRetryAt !== "number") return "Sending again…";
  const seconds = Math.max(0, Math.ceil((nextRetryAt - now) / 1000));
  return `Sending again in ${seconds}s…`;
}

export function recoveryAction(
  action: unknown,
): "retry" | "continue" | null {
  return action === "retry" || action === "continue" ? action : null;
}

export function recoveryTurnOptions(overrides: TurnOptions): TurnOptions {
  const options: TurnOptions = {};
  if (overrides.model !== undefined) options.model = overrides.model;
  if (overrides.permissionMode !== undefined) options.permissionMode = overrides.permissionMode;
  if (overrides.planMode !== undefined) options.planMode = overrides.planMode;
  if (overrides.reasoningLevel !== undefined) options.reasoningLevel = overrides.reasoningLevel;
  return options;
}
