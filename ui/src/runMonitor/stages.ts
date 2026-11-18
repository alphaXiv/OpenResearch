import { m } from "../paraglide/messages.js";
import type { RunModel } from "./model";
import type { Phase, RunMonitor, StageOrder } from "./types";

export function stagePosition(order: StageOrder | null | undefined, settingUp: boolean) {
  if (!order) return null;
  const before = settingUp || order.index !== null && order.first !== null && order.index < order.first;
  return {
    index: before ? 0 : order.index === null || order.first === null ? null : order.index - order.first + 1,
    count: order.count === null || order.first === null ? null : order.count - order.first + 1,
  };
}

/** The phases before the first displayed point share one Setup stage. */
export function foldedStages(monitor: RunMonitor, model: RunModel) {
  if (monitor.phases.length === 0) return null;

  const order = monitor.stageOrder;
  const first = order.first ?? (model.live ? (order.index ?? -1) + 1 : monitor.phases.length);
  const setup = monitor.phases.slice(0, first);
  const ends = setup.flatMap((phase) => phase.endedAt === null ? [] : [phase.endedAt]);
  const ended = ends.length > 0 && setup.every((phase) => phase.skipped || phase.endedAt !== null);
  const end = order.first === null && !model.live ? model.run.endedAt ?? model.run.updatedAt
    : ended ? Math.max(...ends) : null;
  const starts = setup.flatMap((phase) => phase.startedAt === null ? [] : [phase.startedAt]);
  const times = setup.map((phase) => phase.skipped ? 0 : phase.priorMs);
  const phase: Phase = {
    name: m.run_stage_setup(),
    of: null,
    unit: null,
    priorMs: times.length > 0 && times.every((time) => time !== null) ? times.reduce((sum, time) => sum + time, 0) : null,
    startedAt: starts.length > 0 ? Math.min(...starts) : model.pre ? null : model.run.startedAt ?? model.run.createdAt,
    endedAt: end,
    skipped: setup.length > 0 && setup.every((phase) => phase.skipped),
  };
  const position = stagePosition(order, monitor.setup != null);
  const current = model.pre || model.run.status === "done" ? null : position?.index ?? null;
  const names = setup.map((phase) => phase.name).filter((name) => name && name.toLowerCase() !== phase.name.toLowerCase());
  const phases = monitor.phases.slice(first).map((phase) => phase.name ? phase : { ...phase, name: m.run_stage_run() });
  return { phases: [phase, ...phases], current, names };
}
