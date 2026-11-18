import type { LocalMachine } from "./api";
import { fmtMemory } from "./i18n";
import { m } from "./paraglide/messages.js";

export function localMachineFacts(hw: LocalMachine) {
  return {
    processor: hw.chip ?? `${hw.os}/${hw.arch}`,
    cores: hw.cpuCount > 0 ? hw.cpuCount : null,
    memory: hw.memBytes === null ? null : fmtMemory(hw.memBytes),
    gpus: hw.gpus.length,
  };
}

export function localMachineSummary(hw: LocalMachine) {
  const { processor, cores, memory, gpus } = localMachineFacts(hw);
  return [processor, cores === null ? null : m.settings_cpu_cores({ count: cores }), memory, gpus === 0 ? null : m.settings_gpu_count({ count: gpus })]
    .filter(Boolean)
    .join(" · ");
}
