import { useMemo, useRef, useState, type InputHTMLAttributes, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, GripVertical } from "lucide-react";
import { groupModelLines } from "../modelLines";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { mutUpdateUiState } from "../queries/settings";
import { ModelMark } from "./HarnessLogo";
import { bySection, lineName, lineSection, matches, OTHER, SectionMark, sectionName, type PickerLine } from "./ModelList";
import { agentOffers, pickerAgents } from "./pickerAgents";
import { Button, Input, Tooltip } from "./ui";
import { cn } from "./ui/cn";

function Checkbox({ mixed = false, className, ...props }: InputHTMLAttributes<HTMLInputElement> & { mixed?: boolean }) {
  return (
    <input
      type="checkbox"
      ref={(input) => {
        if (input) input.indeterminate = mixed;
      }}
      className={cn("m-0 size-4 shrink-0 cursor-pointer accent-primary focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2", className)}
      {...props}
    />
  );
}

/** Settings → Agents → Models: which models the picker and delegation offer, and the order of the picker's categories. */
export function ModelSettings({ heading }: { heading: ReactNode }) {
  const workspace = useWorkspace();
  const { getHarnessesQuery, getOpenCodeProvidersQuery, getLocalModelsQuery, getUiStateQuery } = workspace.queries;
  const { data: catalog } = useQuery(getHarnessesQuery());
  const { data: providers } = useQuery(getOpenCodeProvidersQuery());
  const { data: endpoints } = useQuery(getLocalModelsQuery());
  const { data: uiState } = useQuery(getUiStateQuery());
  const [filter, setFilter] = useState("");
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [dragged, setDragged] = useState<{ id: string; order: string[]; from: string[] }>();
  const listRef = useRef<HTMLDivElement>(null);

  const lines = useMemo(
    () => groupModelLines(pickerAgents(catalog ?? [], providers ?? [], endpoints ?? []).flatMap((agent) => agentOffers(agent))),
    [catalog, providers, endpoints],
  );
  if (!uiState || !lines.length) return null;

  const hidden = new Set(uiState.hiddenModels);
  const words = filter.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const sections = new Map<string, PickerLine[]>();
  for (const line of bySection(lines, dragged?.order ?? uiState.modelSectionOrder)) {
    const id = lineSection(line);
    sections.set(id, [...(sections.get(id) ?? []), line]);
  }
  const movable = [...sections.keys()].filter((id) => id !== OTHER);

  const moved = (id: string, to: number) => {
    const order = movable.filter((other) => other !== id);
    order.splice(Math.max(0, Math.min(to, order.length)), 0, id);
    return order;
  };

  const isOn = (line: PickerLine) => !hidden.has(line.key);

  const toggle = (targets: PickerLine[], on: boolean) => {
    const ids = targets.map((line) => line.key);
    mutUpdateUiState.run(workspace, { hiddenModels: on ? uiState.hiddenModels.filter((id) => !ids.includes(id)) : [...new Set([...uiState.hiddenModels, ...ids])] });
  };

  // A grip drags its category among the others; where the pointer passes a header's middle, the category moves past it.
  const drag = (id: string) => ({
    onPointerDown: (event: PointerEvent<HTMLElement>) => {
      event.currentTarget.setPointerCapture(event.pointerId);
      setDragged({ id, order: movable, from: movable });
    },
    onPointerMove: (event: PointerEvent<HTMLElement>) => {
      if (!dragged) return;
      const headers = [...(listRef.current?.querySelectorAll<HTMLElement>("[data-section]") ?? [])].filter((header) => header.dataset.section !== id);
      const before = headers.filter((header) => {
        const rect = header.getBoundingClientRect();
        return rect.top + rect.height / 2 < event.clientY;
      }).length;
      if (dragged.order.indexOf(id) !== before) setDragged({ ...dragged, order: moved(id, before) });
    },
    onPointerUp: () => {
      if (dragged && dragged.order.join() !== dragged.from.join()) mutUpdateUiState.run(workspace, { modelSectionOrder: dragged.order });
      setDragged(undefined);
    },
    onPointerCancel: () => setDragged(undefined),
    onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
      const step = event.key === "ArrowUp" ? -1 : event.key === "ArrowDown" ? 1 : 0;
      if (!step) return;
      event.preventDefault();
      mutUpdateUiState.run(workspace, { modelSectionOrder: moved(id, movable.indexOf(id) + step) });
    },
  });

  const shown = [...sections].map(([id, lines]) => [id, lines.filter((line) => matches(line, words))] as const)
    .filter(([, lines]) => lines.length > 0);
  const listed = shown.flatMap(([, lines]) => lines);

  return (
    <>
      {heading}
      <p className="mt-0 mb-3 text-sm text-subtext">{m.settings_models_description()}</p>
      <Input
        type="search"
        value={filter}
        aria-label={m.model_picker_search_models()}
        placeholder={m.model_picker_search_models()}
        onChange={(event) => setFilter(event.target.value)}
        className="mb-2"
      />
      <div className="mb-2 flex gap-2">
        <Button size="small" disabled={listed.every(isOn)} onClick={() => toggle(listed, true)}>{m.settings_models_all_on()}</Button>
        <Button size="small" disabled={!listed.some(isOn)} onClick={() => toggle(listed, false)}>{m.settings_models_all_off()}</Button>
      </div>
      <div ref={listRef} className="overflow-hidden rounded-lg border border-border">
        {shown.length === 0 && <p className="m-0 px-3 py-2.5 text-sm text-subtext">{m.add_agent_no_matches({ query: filter.trim() })}</p>}
        {shown.map(([id, lines]) => {
          const open = words.length > 0 || expanded.has(id);
          const on = lines.filter(isOn).length;
          const off = lines.length - on;
          const name = sectionName(id);
          return (
            <div key={id} className="border-b border-border-variant last:border-b-0">
              <div data-section={id} className={cn("flex min-h-10 items-center gap-1 ps-1 pe-2", dragged?.id === id && "bg-hover-muted")}>
                {id !== OTHER && !words.length ? (
                  <Tooltip interactive content={m.settings_models_reorder()} className="rounded-md">
                    <button
                      type="button"
                      aria-label={m.settings_models_move({ name })}
                      aria-keyshortcuts="ArrowUp ArrowDown"
                      className="grid h-8 w-6 cursor-grab touch-none place-items-center rounded-md text-muted outline-none hover:text-text focus-visible:outline-2 focus-visible:outline-focus active:cursor-grabbing"
                      {...drag(id)}
                    >
                      <GripVertical size={14} aria-hidden="true" />
                    </button>
                  </Tooltip>
                ) : (
                  <span className="w-6 shrink-0" />
                )}
                <label className="grid size-6 shrink-0 cursor-pointer place-items-center rounded-md">
                  <Checkbox
                    checked={off === 0}
                    mixed={on > 0 && off > 0}
                    aria-label={m.settings_models_lab_all({ name })}
                    onChange={(event) => toggle(lines, event.target.checked)}
                  />
                </label>
                <button
                  type="button"
                  aria-expanded={open}
                  disabled={words.length > 0}
                  onClick={() => setExpanded((current) => {
                    const next = new Set(current);
                    if (!next.delete(id)) next.add(id);
                    return next;
                  })}
                  className="flex min-w-0 flex-1 items-center gap-2 self-stretch rounded-md ps-1 text-start text-sm outline-none focus-visible:outline-2 focus-visible:outline-focus"
                >
                  <SectionMark id={id} size={16} />
                  <span className="min-w-0 flex-1 truncate font-medium">{name}</span>
                  {off > 0 && <span className="shrink-0 text-muted">{m.settings_models_off({ count: off })}</span>}
                  {!words.length && <ChevronRight size={14} aria-hidden="true" className={cn("shrink-0 text-muted motion-safe:transition-transform", open ? "rotate-90" : "rtl:rotate-180")} />}
                </button>
              </div>
              {open && lines.map((line) => (
                <label key={line.key} className="flex min-h-9 cursor-pointer items-center gap-2 ps-16 pe-3 hover:bg-hover-muted">
                  <Checkbox checked={isOn(line)} aria-label={lineName(line)} onChange={(event) => toggle([line], event.target.checked)} />
                  <ModelMark model={line.offers[0].model} harness={line.offers[0].tile.harness.id} size={16} />
                  <span className={cn("min-w-0 truncate text-sm", !isOn(line) && "text-muted")}>{lineName(line)}</span>
                </label>
              ))}
            </div>
          );
        })}
      </div>
    </>
  );
}
