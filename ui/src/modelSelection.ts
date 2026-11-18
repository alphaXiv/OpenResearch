import { modelLabel, reconcileReasoning, reconcileServiceTier, tidyModelName, type AgentSelection, type Harness, type HarnessModel, type OptionChoice } from "./api";
import { groupModelLines } from "./modelLines.ts";
import { fmtNumber } from "./i18n";
import { m } from "./paraglide/messages.js";
import { getLocale } from "./paraglide/runtime.js";

/** The entry a selection or chat runs on: its agent, and the account when it names one. */
export function selectionHarness(harnesses: Harness[], selection: Pick<AgentSelection, "harness" | "profile"> | null | undefined): Harness | undefined {
  return selection ? harnesses.find((h) => h.id === selection.harness && (h.profile?.id ?? null) === (selection.profile ?? null)) : undefined;
}

/** Tells entries apart: an account's id never matches an agent's. */
export function harnessKey(h: Harness): string {
  return h.profile?.id ?? h.id;
}

/** An agent OpenResearch installed for itself. Its copies of Claude Code, Codex and OpenCode stand in for the user's own. */
export function keptInstall(h: Harness): boolean {
  return Boolean(h.managed && !h.profile && !["claude-code", "codex", "opencode"].includes(h.id));
}

/** "1 model available", "3 models available". */
export function modelsAvailable(count: number) {
  const message = new Intl.PluralRules(getLocale()).select(count) === "one" ? m.settings_models_available_one : m.settings_models_available;
  return message({ count: fmtNumber(count) });
}

/** An agent's models as the picker lists them: one line per model, older versions last. */
export function agentModelLines(harness: string, models: HarnessModel[]) {
  const lines = groupModelLines(models.filter((model) => !model.parsed.superseded).map((model) => ({ tile: harness, model })));
  return [...lines.filter((line) => !line.older), ...lines.filter((line) => line.older)];
}

/** OpenCode's Auto names who reviews: the chat's own model, or the one picked in Settings. A chat on a local model
 * (one a local server was connected for) always reviews with its own. */
export function withAutoReviewer(choices: OptionChoice[], harness: Harness | undefined, reviewModel: string | null | undefined, chatModel: string | null | undefined): OptionChoice[] {
  if (harness?.id !== "opencode") return choices;
  const named = reviewModel && !chatModel?.startsWith("orx-local-") && tidyModelName(harness.models.find((model) => model.id === reviewModel)?.displayName ?? modelLabel(reviewModel));
  return choices.map((choice) => choice.id !== "auto" ? choice : { ...choice, description: named ? m.opencode_auto_model({ model: named }) : m.opencode_auto_same_model() });
}

/** An account OpenResearch keeps reads "Claude Code · Work", or its own name when that already says the agent. */
export function harnessDisplayName(h: Harness): string {
  if (!h.profile) return h.label?.name ?? h.name;
  return h.profile.name.toLowerCase().includes(h.name.toLowerCase()) ? h.profile.name : `${h.name} · ${h.profile.name}`;
}

/** Reconciles a selection's speed and effort with its model. A model the
 * catalog no longer lists stays as saved: an open chat keeps running it. */
export function deriveComposerSelection(
  rawSelection: AgentSelection | null,
  activeHarness: Harness | undefined,
): AgentSelection | null {
  if (!rawSelection) return null;
  const readyHarness = activeHarness?.catalogPending || (activeHarness?.id === "codex" && !activeHarness.models.length)
    ? undefined
    : activeHarness;
  return {
    ...rawSelection,
    serviceTier: reconcileServiceTier(readyHarness, rawSelection.model, rawSelection.serviceTier),
    reasoningLevel: reconcileReasoning(readyHarness, rawSelection.model, rawSelection.reasoningLevel),
  };
}
