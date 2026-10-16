import type { HarnessModel, OptionChoice } from "./api";

export interface ProviderGroup extends OptionChoice {
  /** Whether any model is reachable this way yet (a key set, a plan signed in). */
  available: boolean;
}

/** The provider groups of a harness: the ones it declares (offered even before
 * they have models), then any other group its models carry, in catalog order. */
export function providerGroups(
  models: readonly HarnessModel[],
  declared: readonly OptionChoice[] = [],
): ProviderGroup[] {
  const seen = new Map<string, OptionChoice>();
  for (const group of declared) if (!seen.has(group.id)) seen.set(group.id, group);
  for (const entry of models) {
    if (entry.providerGroup && !seen.has(entry.providerGroup.id)) seen.set(entry.providerGroup.id, entry.providerGroup);
  }
  return [...seen.values()].map((group) => ({
    ...group,
    available: models.some((entry) => entry.providerGroup?.id === group.id),
  }));
}

/** The model a provider switch lands on: the same model under the new
 * provider when it has one (by display name), else its first model. */
export function providerSwitchTarget(
  models: readonly HarnessModel[],
  current: string | null | undefined,
  group: string,
  label: (model: HarnessModel) => string,
): string | undefined {
  const candidates = models.filter((entry) => entry.providerGroup?.id === group);
  const selected = models.find((entry) => entry.id === current);
  const same = selected && candidates.find((entry) => label(entry) === label(selected));
  return (same ?? candidates[0])?.id;
}
