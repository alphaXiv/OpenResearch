import type { LocalMachine } from "./api";
import { fmtMemory } from "./i18n";
import { m } from "./paraglide/messages.js";

export function localMachineSummary(hw: LocalMachine) {
  const processor = hw.chip ?? `${hw.os}/${hw.arch}`;
  const memory = hw.memBytes === null ? null : fmtMemory(hw.memBytes);
  const gpu = hw.gpus.length === 0 ? null : m.settings_gpu_count({ count: hw.gpus.length });
  return [processor, hw.cpuCount > 0 ? m.settings_cpu_cores({ count: hw.cpuCount }) : null, memory, gpu]
    .filter(Boolean)
    .join(" · ");
}
