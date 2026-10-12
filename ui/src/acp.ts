import type { ChatSession, Harness, OptionChoice } from "./api";

export interface NativeOption {
  id: string;
  name: string;
  category?: string;
  currentValue: string;
  choices: OptionChoice[];
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function choices(values: unknown, group?: string): OptionChoice[] {
  if (!Array.isArray(values)) return [];
  return values.flatMap((value: unknown) => {
    if (!record(value)) return [];
    if (Array.isArray(value.options)) return choices(value.options, typeof value.name === "string" ? value.name : undefined);
    if (typeof value.value !== "string" || typeof value.name !== "string") return [];
    return [{ id: value.value, label: value.name, description: group ?? (typeof value.description === "string" ? value.description : undefined) }];
  });
}

export function nativeOptions(configuration: unknown, labels = { model: "Model", mode: "Mode" }): NativeOption[] {
  if (!record(configuration)) return [];
  if (Array.isArray(configuration.configOptions)) {
    return configuration.configOptions.flatMap((option: unknown) => {
      if (!record(option) || option.type !== "select" || typeof option.id !== "string" || typeof option.name !== "string" || typeof option.currentValue !== "string") return [];
      return [{ id: option.id, name: option.name, category: typeof option.category === "string" ? option.category : undefined, currentValue: option.currentValue, choices: choices(option.options) }];
    });
  }
  return ["models", "modes"].flatMap((section) => {
    const value = configuration[section];
    if (!record(value)) return [];
    const model = section === "models";
    const available = value[model ? "availableModels" : "availableModes"];
    const current = value[model ? "currentModelId" : "currentModeId"];
    if (!Array.isArray(available) || typeof current !== "string") return [];
    const options = available.flatMap((option: unknown) => {
      if (!record(option)) return [];
      const id = option[model ? "modelId" : "id"];
      if (typeof id !== "string" || typeof option.name !== "string") return [];
      return [{ id, label: option.name }];
    });
    return [{ id: model ? "model" : "mode", name: model ? labels.model : labels.mode, category: model ? "model" : "mode", currentValue: current, choices: options }];
  });
}

export function sessionAcpHarness(session: ChatSession | null | undefined, catalog: Harness | undefined): Harness | undefined {
  if (!session?.harness.startsWith("acp:")) return catalog;
  const model = nativeOptions(session.nativeConfiguration).find((option) => option.category === "model");
  return {
    id: session.harness, name: session.harnessName ?? catalog?.name ?? "ACP", installed: true,
    installBroken: false, authenticated: false, authState: "unknown", agentReady: true, supportsSteering: false,
    models: model ? model.choices.map((choice) => ({ id: choice.id, displayName: choice.label, description: choice.description })) : [{ id: "default" }],
    options: { permissionModes: [], reasoningLevels: [], defaultPermissionMode: null, defaultReasoningLevel: null },
  };
}

export function validAcpLaunch(name: string, executable: string, args: string[]): boolean {
  return name.trim().length > 0 && executable.trim().length > 0
    && ![name, executable, ...args].some((value) => value.includes("\0"))
    && (!/[\\/]/.test(executable) || executable.startsWith("/") || /^[A-Za-z]:[\\/]/.test(executable));
}
