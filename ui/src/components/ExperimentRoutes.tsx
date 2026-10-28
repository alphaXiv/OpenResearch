import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, GitFork } from "lucide-react";
import { runDisplayStatus, type Experiment, type Run } from "../api";
import { buildRoutes, parseRouteFolds, routeRows, routeRun, visibleRouteId } from "../experimentRoutes";
import { m } from "../paraglide/messages.js";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { StatusBadge } from "./StatusBadge";
import { WorkspaceEmptyState } from "./WorkspaceEmptyState";

export function ExperimentRoutes({ experiments, runs, sessionId, showArchived, storageKey, emptyHint, onOpen }: {
  experiments: Experiment[];
  runs: Run[];
  sessionId: string | null;
  showArchived: boolean;
  storageKey: string;
  emptyHint?: string;
  onOpen: (id: string, intent: TabOpenIntent, runId?: string) => void;
}) {
  const [collapsed, setCollapsed] = useState(() => {
    try { return parseRouteFolds(JSON.parse(localStorage.getItem(storageKey) ?? "null")); }
    catch { return new Set<string>(); }
  });
  const [selectedId, setSelectedId] = useState<string | null>(() => {
    try { return localStorage.getItem(`${storageKey}:selection`); } catch { return null; }
  });
  const [focusedId, setFocusedId] = useState<string | null>(selectedId);
  const treeRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const routes = useMemo(() => buildRoutes(experiments, sessionId, showArchived), [experiments, sessionId, showArchived]);
  const rows = useMemo(() => routeRows(routes, collapsed), [routes, collapsed]);
  const focusId = visibleRouteId(focusedId, routes, rows);
  const selection = selectedId && routes.nodes.has(selectedId) ? visibleRouteId(selectedId, routes, rows) : null;
  const runsByExperiment = useMemo(() => {
    const grouped = new Map<string, Run[]>();
    for (const run of runs) {
      const group = grouped.get(run.experimentId) ?? [];
      group.push(run);
      grouped.set(run.experimentId, group);
    }
    return grouped;
  }, [runs]);
  useEffect(() => {
    try { localStorage.setItem(storageKey, JSON.stringify([...collapsed])); } catch { /* Browser storage may be unavailable. */ }
  }, [storageKey, collapsed]);
  useEffect(() => {
    if (focusedId !== focusId && (treeRef.current?.contains(document.activeElement) || document.activeElement === document.body)) rowRefs.current.get(focusId ?? "")?.focus();
  }, [focusedId, focusId]);
  const focus = (id: string) => {
    setFocusedId(id);
    rowRefs.current.get(id)?.focus();
  };
  const toggle = (id: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
    focus(id);
  };
  if (!rows.length) return <WorkspaceEmptyState icon={GitFork} title={emptyHint ?? m.experiments_none_yet()} description={emptyHint ? undefined : m.experiments_empty_description()} />;
  return (
    <div className="absolute inset-0 overflow-auto bg-background p-2">
      <div ref={treeRef} role="tree" aria-label={m.app_routes()} className="text-sm text-text" style={{ minWidth: 260 + rows.reduce((max, row) => Math.max(max, row.depth), 0) * 16 }}>
        {rows.map(({ node, depth, position, size, continuation }, index) => {
          const experiment = node.experiment;
          const id = experiment.id;
          const title = experiment.title || experiment.slug;
          const run = routeRun(runsByExperiment.get(id) ?? []);
          const status = run ? runDisplayStatus(run) : experiment.agentStatus === "editing" ? "editing" : "idle";
          const folded = collapsed.has(id);
          const gestures = tabOpenGestureHandlers<HTMLDivElement>((intent) => {
            setSelectedId(id);
            try { localStorage.setItem(`${storageKey}:selection`, id); } catch { /* Browser storage may be unavailable. */ }
            onOpen(id, intent, run?.id);
          });
          const notes = [
            node.context && (experiment.archived && !showArchived ? m.routes_archived_ancestor() : m.routes_other_task()),
            node.boundary === "missing" && m.routes_missing_parent(),
            node.boundary === "cycle" && m.routes_invalid_lineage(),
          ].filter(Boolean).join(" · ");
          return (
            <div key={id} ref={(element) => { if (element) rowRefs.current.set(id, element); else rowRefs.current.delete(id); }}
              role="treeitem" aria-level={depth + 1} aria-posinset={position} aria-setsize={size}
              aria-expanded={node.children.length ? !folded : undefined} aria-selected={selection === id}
              tabIndex={focusId === id ? 0 : -1} title={[title, notes].filter(Boolean).join(" · ")}
              className={`route-row flex h-10 items-center rounded-md pe-2 hover:bg-surface focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 ${selection === id ? "bg-panel" : ""}`}
              {...gestures} onFocus={() => setFocusedId(id)}
              onKeyDown={(event) => {
                let next: string | undefined;
                if (event.key === "ArrowDown") next = rows[Math.min(index + 1, rows.length - 1)].node.experiment.id;
                else if (event.key === "ArrowUp") next = rows[Math.max(index - 1, 0)].node.experiment.id;
                else if (event.key === "Home") next = rows[0].node.experiment.id;
                else if (event.key === "End") next = rows[rows.length - 1].node.experiment.id;
                else if (event.key === "ArrowRight") {
                  if (node.children.length && folded) toggle(id); else next = node.children[0];
                } else if (event.key === "ArrowLeft") {
                  if (node.children.length && !folded) toggle(id); else next = node.parentId ?? undefined;
                } else { gestures.onKeyDown(event); return; }
                event.preventDefault();
                if (next) focus(next);
              }}>
              <span aria-hidden="true" className="flex h-full shrink-0">
                {continuation.map((continues, level) => <span key={level} className="relative w-4">
                  {level === depth - 1 ? <><span className={`absolute start-2 top-0 border-s border-border ${position < size ? "h-full" : "h-1/2"}`} /><span className="absolute start-2 top-1/2 w-2 border-t border-border" /></>
                    : continues && <span className="absolute start-2 top-0 h-full border-s border-border" />}
                </span>)}
              </span>
              <button type="button" tabIndex={-1} disabled={!node.children.length}
                className="flex size-6 shrink-0 items-center justify-center rounded text-subtext hover:bg-highlight disabled:invisible"
                aria-label={folded ? m.a11y_expand_item({ name: title }) : m.a11y_collapse_item({ name: title })}
                onClick={(event) => { event.stopPropagation(); toggle(id); }} onDoubleClick={(event) => event.stopPropagation()} onAuxClick={(event) => event.stopPropagation()}>
                {folded ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
              </button>
              <span className="flex min-w-0 flex-1 flex-col pe-3">
                <span className={`truncate ${node.context ? "text-subtext" : "text-text"}`}>{title}</span>
                {notes && <span className="truncate text-xs text-muted">{notes}</span>}
              </span>
              <StatusBadge status={status} className="shrink-0" />
            </div>
          );
        })}
      </div>
    </div>
  );
}
