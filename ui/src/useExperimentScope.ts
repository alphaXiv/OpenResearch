import { useCallback, useMemo, useRef, useState } from "react";

import { usePopover } from "./components/ModelPicker";
import { type Experiment, type Run } from "./api";

export type ExperimentsView = "tree" | "table";

/** The experiments pane's view toggle (tree/table) and its "agent" vs
 * "project" scope filter — "agent" narrows to the open chat session's work,
 * falling back to "project" whenever there is no usable experiment
 * attribution. */
export function useExperimentScope(experiments: Experiment[], runs: Run[], activeSessionId: string | null) {
  const [view, setView] = useState<ExperimentsView>("table");
  // Experiments pane scope: "agent" narrows to the open chat session's work.
  // Falls back to "project" whenever there is no usable experiment attribution.
  const [scope, setScope] = useState<"agent" | "project">("project");
  const scopeTriggerRef = useRef<HTMLButtonElement>(null);
  const { open: scopeMenuOpen, setOpen: setScopeMenuOpen, ref: scopeMenuRef } =
    usePopover(scopeTriggerRef);
  const allExperimentsAttributed = experiments.every((experiment) => experiment.chatSessionId);
  const effectiveScope = activeSessionId && allExperimentsAttributed ? scope : "project";
  const scopedExperiments = useMemo(() => {
    if (effectiveScope !== "agent") return experiments;
    return experiments.filter((experiment) => experiment.chatSessionId === activeSessionId);
  }, [experiments, effectiveScope, activeSessionId]);
  // Runs are scoped by their experiment's owner, not by which session launched them.
  const scopedRuns = useMemo(() => {
    if (effectiveScope !== "agent") return runs;
    const mine = new Set(scopedExperiments.map((experiment) => experiment.id));
    return runs.filter((r) => mine.has(r.experimentId));
  }, [runs, scopedExperiments, effectiveScope]);

  // Stable identity: in TreeView's layout-memo deps, so an inline arrow would
  // recompute the graph on every render.
  const showProjectScope = useCallback(() => setScope("project"), []);

  return {
    view, setView,
    scope, setScope,
    scopeTriggerRef, scopeMenuOpen, setScopeMenuOpen, scopeMenuRef,
    allExperimentsAttributed, effectiveScope,
    scopedExperiments, scopedRuns,
    showProjectScope,
  };
}
