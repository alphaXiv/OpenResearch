import type { TurnOptions } from "./api";
import { m } from "./paraglide/messages.js";
import { getLocale } from "./paraglide/runtime.js";

const fmtNumber = (value: number) => new Intl.NumberFormat(getLocale()).format(value);

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
    return m.retry_cli();
  }
  if (typeof input.attempt === "number" && typeof input.maximum === "number" && seconds != null) {
    return m.retry_attempt_max_seconds({ attempt: fmtNumber(input.attempt), maximum: fmtNumber(input.maximum), seconds: fmtNumber(seconds) });
  }
  if (typeof input.attempt === "number" && typeof input.maximum === "number") {
    return m.retry_attempt_max({ attempt: fmtNumber(input.attempt), maximum: fmtNumber(input.maximum) });
  }
  if (typeof input.attempt === "number" && seconds != null) {
    return m.retry_attempt_seconds({ attempt: fmtNumber(input.attempt), seconds: fmtNumber(seconds) });
  }
  if (typeof input.attempt === "number") return m.retry_attempt({ attempt: fmtNumber(input.attempt) });
  if (seconds != null) return m.retry_seconds({ seconds: fmtNumber(seconds) });
  return m.retrying();
}

export function queuedRetryLabel(nextRetryAt: number | null | undefined, now: number): string {
  if (typeof nextRetryAt !== "number") return m.retry_sending_again();
  const seconds = Math.max(0, Math.ceil((nextRetryAt - now) / 1000));
  return m.retry_sending_again_seconds({ seconds: fmtNumber(seconds) });
}

export function recoveryAction(
  action: unknown,
): "retry" | "continue" | null {
  return action === "retry" || action === "continue" ? action : null;
}

export function recoveryTurnOptions(overrides: TurnOptions): TurnOptions {
  const options: TurnOptions = {};
  if (overrides.model !== undefined) options.model = overrides.model;
  if (overrides.serviceTier !== undefined) options.serviceTier = overrides.serviceTier;
  if (overrides.permissionMode !== undefined) options.permissionMode = overrides.permissionMode;
  if (overrides.planMode !== undefined) options.planMode = overrides.planMode;
  if (overrides.reasoningLevel !== undefined) options.reasoningLevel = overrides.reasoningLevel;
  return options;
}
