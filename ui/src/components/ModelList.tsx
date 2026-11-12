import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { flushSync } from "react-dom";
import { Check, ChevronRight, History, Sparkles, Star } from "lucide-react";
import { fmtNumber, tidyModelName } from "../api";
import { m } from "../paraglide/messages.js";
import { CLAUDE_TIERS, type ModelLine, type ModelOffer } from "../modelLines";
import { harnessKey } from "../modelSelection";
import { AgentIconTile } from "./AgentIcon";
import { agentLab, LabMark, LABS, ModelMark, modelLab, type Lab } from "./HarnessLogo";
import type { PickerAgent } from "./pickerAgents";
import { Tooltip } from "./ui";
import { cn } from "./ui/cn";

export type PickerLine = ModelLine<PickerAgent>;
type PickerOffer = ModelOffer<PickerAgent>;

/** A section's title bar: stuck, it spans the list's padding and covers the rows scrolling under it. */
const BAR = "sticky -top-1 z-10 -mx-1 bg-background px-1 after:absolute after:inset-x-0 after:top-full after:h-px data-stuck:after:bg-border-variant";
const HEADER = "flex h-7 w-full items-center gap-1.5 rounded-md ps-2.5 pe-2 pt-1 text-xs font-medium text-muted outline-none";
const ROW = "flex min-h-8 items-center gap-1 rounded-md ps-2 pe-1 hover:bg-hover-muted has-[[data-nav]:focus-visible]:bg-hover-muted";
const MAIN = "flex min-w-0 flex-1 items-center gap-2 self-stretch text-start text-sm outline-none";
const BADGE = "shrink-0 rounded-sm px-1 text-xs leading-4";

/** A catalog this long (OpenRouter's) lists only what another agent offers too until a search finds the rest. */
const CATALOG_LINES = 60;

const NEW = "new";
const FAVORITES = "favorites";
export const OTHER = "other";
const OLDER = "older";

interface Section {
  id: string;
  name: string;
  mark: (size: number) => ReactNode;
  lines: PickerLine[];
}

export function lineLab(line: PickerLine) {
  const [offer] = line.offers;
  return modelLab(offer.model, offer.tile.harness.id);
}

/** Where a lab's section goes: the user's order first, then the labs it doesn't name in `LABS` order, Other models last. */
function labRank(lab: Lab | undefined, order: readonly string[]) {
  if (!lab) return Infinity;
  const chosen = order.indexOf(lab);
  return chosen >= 0 ? chosen : order.length + LABS.findIndex((entry) => entry.lab === lab);
}

export function labName(lab: Lab | undefined) {
  return LABS.find((entry) => entry.lab === lab)?.name ?? m.model_picker_other();
}

/** Lines by lab in section order, Claude's biggest tier first, otherwise as grouped. */
export function byLab(lines: PickerLine[], order: readonly string[]) {
  const ranks = new Map(lines.map((line, index) => {
    const words = line.family.split(" ");
    return [line, [labRank(lineLab(line), order), CLAUDE_TIERS.findIndex((tier) => words.includes(tier)), index]];
  }));
  return [...lines].sort((a, b) => {
    const [x, y] = [ranks.get(a)!, ranks.get(b)!];
    return x[0] - y[0] || x[1] - y[1] || x[2] - y[2];
  });
}

/**
 * Where a click on a model's name runs it: the agent it runs on now, else one of OpenCode's,
 * else the lab's own agent, else the first that offers it.
 */
function autoAgent(line: PickerLine, agents: PickerAgent[], current: PickerAgent | undefined) {
  const lab = lineLab(line);
  return (current && agents.includes(current) ? current : undefined)
    ?? agents.find((agent) => agent.harness.id === "opencode")
    ?? agents.find((agent) => lab && agentLab(agent.harness.id, agent.harness.logo) === lab)
    ?? agents[0];
}

/** The agents offering `line`, in Settings' order. */
function lineAgents(line: PickerLine, agents: PickerAgent[]) {
  return agents.filter((agent) => line.offers.some((offer) => offer.tile === agent));
}

export function lineName(line: PickerLine) {
  return tidyModelName(line.label);
}

/** Whether `line` has every one of the lowercase search `words` in its name, its models' ids or its agents' names. */
export function matches(line: PickerLine, words: string[]) {
  const text = [line.label, line.key, ...line.offers.flatMap((offer) => [offer.model.id, offer.tile.name])].join(" ").toLowerCase();
  return words.every((word) => text.includes(word));
}

/** The models, one collapsible section per lab, beside a rail that jumps between sections; a search lists its matches instead. */
export function ModelList({
  lines,
  agents,
  filter,
  onClearFilter,
  favorites,
  fresh,
  order,
  collapsed: initiallyCollapsed,
  onCollapse,
  selectedLine,
  selectedAgent,
  lockedHarness,
  onPick,
  onToggleFavorite,
  children,
}: {
  lines: PickerLine[];
  agents: PickerAgent[];
  filter: string;
  onClearFilter: () => void;
  favorites: ReadonlySet<string>;
  /** Keys of the models agents newly offer. */
  fresh: ReadonlySet<string>;
  order: readonly string[];
  collapsed: readonly string[];
  onCollapse: (collapsed: string[]) => void;
  selectedLine?: PickerLine;
  selectedAgent?: PickerAgent;
  /** The agent the chat can't leave right now, by `harnessKey`: other agents' models open a new chat instead. */
  lockedHarness?: string;
  onPick: (line: PickerLine, agent: PickerAgent) => void;
  onToggleFavorite: (family: string) => void;
  /** Rows after the models. */
  children?: ReactNode;
}) {
  const uid = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const railRef = useRef<HTMLDivElement>(null);
  const jumping = useRef(false);
  const starred = useRef<{ star: HTMLElement; top: number }>(undefined);
  const [collapsed, setCollapsed] = useState(() => new Set(initiallyCollapsed));
  const [olderOpen, setOlderOpen] = useState(false);
  const [spied, setSpied] = useState<string>();
  const [stuck, setStuck] = useState<string>();

  const words = filter.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const searching = words.length > 0;
  const counts = new Map(agents.map((agent) => [agent, lines.filter((line) => line.offers.some((offer) => offer.tile === agent)).length]));
  const listed = (line: PickerLine) =>
    line === selectedLine || favorites.has(line.family) || fresh.has(line.key) || lineAgents(line, agents).some((agent) => counts.get(agent)! <= CATALOG_LINES);

  const ordered = byLab(lines, order);
  const current = ordered.filter((line) => (!line.older || line === selectedLine) && listed(line));
  const unlisted = ordered.filter((line) => !listed(line)).length;

  const labs: Section[] = [];
  for (const line of current) {
    const lab = lineLab(line);
    const section = labs.find((entry) => entry.id === (lab ?? OTHER));
    if (section) section.lines.push(line);
    else labs.push({ id: lab ?? OTHER, name: labName(lab), mark: (size) => <LabMark lab={lab} size={size} />, lines: [line] });
  }
  const older = ordered.filter((line) => line.older && line !== selectedLine && listed(line));
  const olderVersions = (lines: PickerLine[]): Section => ({ id: OLDER, name: m.model_picker_older_versions(), mark: (size) => <History size={size} aria-hidden="true" />, lines });
  const sections = [
    {
      id: NEW,
      name: m.model_picker_new(),
      mark: (size: number) => <Sparkles size={size} aria-hidden="true" className="text-accent-blue-strong" />,
      lines: current.filter((line) => fresh.has(line.key)),
    },
    {
      id: FAVORITES,
      name: m.model_picker_favorites(),
      mark: (size: number) => <Star size={size} aria-hidden="true" className="fill-current text-accent-amber" />,
      lines: current.filter((line) => favorites.has(line.family)),
    },
    ...labs,
    olderVersions(older),
  ].filter((section) => section.lines.length > 0);

  const found = searching ? ordered.filter((line) => matches(line, words)) : [];
  const foundOlder = olderVersions(found.filter((line) => line.older));
  const active = searching ? undefined : sections.some((section) => section.id === spied) ? spied : sections[0]?.id;
  const isOpen = (id: string) => searching || (id === OLDER ? olderOpen : !collapsed.has(id));

  useEffect(() => {
    railRef.current?.querySelector(`[data-rail="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  /** The section whose header sits at the top of the list, and whether rows scroll under it there. */
  const spy = () => {
    const list = listRef.current;
    if (!list) return;
    let top: HTMLElement | undefined;
    for (const element of list.querySelectorAll<HTMLElement>("[data-section]")) {
      if (element.offsetTop <= list.scrollTop + 1) top = element;
    }
    setStuck(top && top.offsetTop < list.scrollTop ? top.dataset.section : undefined);
    if (!jumping.current && !searching) setSpied(top?.dataset.section ?? sections[0]?.id);
  };

  /** Scrolls the list to `top` without spying the scroll it causes. */
  const scrollTo = (top: number) => {
    const list = listRef.current;
    if (!list) return;
    jumping.current = true;
    list.scrollTop = top;
    requestAnimationFrame(() => { jumping.current = false; });
  };

  // A new search starts at the top, and so does the list it leaves.
  useLayoutEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
    spy();
  }, [filter]);

  // A star adds its row to Favorites above, or takes it away: the row stays under the pointer.
  useLayoutEffect(() => {
    const held = starred.current;
    starred.current = undefined;
    if (held?.star.isConnected && listRef.current) scrollTo(listRef.current.scrollTop + held.star.getBoundingClientRect().top - held.top);
  }, [favorites]);

  const toggle = (id: string, open = !isOpen(id)) => {
    if (id === OLDER) return setOlderOpen(open);
    const next = new Set(collapsed);
    if (open) next.delete(id);
    else next.add(id);
    setCollapsed(next);
    onCollapse([...next]);
  };

  const jump = (id: string) => {
    flushSync(() => {
      if (!isOpen(id)) toggle(id, true);
      if (searching) onClearFilter();
    });
    const target = listRef.current?.querySelector<HTMLElement>(`[data-section="${id}"]`);
    if (!target) return;

    // The jump's own scroll event lands before the next frame; a short last section never reaches the top to be spied.
    scrollTo(target.offsetTop);
    setSpied(id);
  };

  const star = (family: string, event: MouseEvent<HTMLElement>) => {
    starred.current = { star: event.currentTarget, top: event.currentTarget.getBoundingClientRect().top };
    onToggleFavorite(family);
  };

  const row = (line: PickerLine, section?: Section) => (
    <ModelRow
      key={line.key}
      line={line}
      agents={lineAgents(line, agents)}
      selected={line === selectedLine}
      currentAgent={selectedAgent}
      selectedAgent={line === selectedLine ? selectedAgent : undefined}
      lockedHarness={lockedHarness}
      favorite={favorites.has(line.family)}
      fresh={fresh.has(line.key) && !line.older && section?.id !== NEW}
      onPick={onPick}
      onToggleFavorite={star}
    />
  );

  const group = (section: Section) => {
    const header = `${uid}${section.id}`;
    const open = isOpen(section.id);
    return (
      <div key={section.id} role="group" aria-labelledby={header} data-section={section.id}>
        <div data-stuck={section.id === stuck || undefined} className={BAR}>
          {searching ? (
            <div id={header} className={HEADER}>
              {section.mark(12)}
              {section.name}
            </div>
          ) : (
            <button
              id={header}
              type="button"
              data-nav
              aria-expanded={open}
              onClick={() => toggle(section.id)}
              className={cn(HEADER, "text-start hover:text-text focus-visible:bg-hover-muted focus-visible:text-text")}
            >
              {section.mark(12)}
              <span className="truncate">{section.name}</span>
              <ChevronRight size={12} aria-hidden="true" className={cn("shrink-0 motion-safe:transition-transform", open ? "rotate-90" : "rtl:rotate-180")} />
            </button>
          )}
        </div>
        {open && section.lines.map((line) => row(line, section))}
      </div>
    );
  };

  return (
    <div className="flex min-h-0 flex-1">
      {sections.length > 1 && (
        <div ref={railRef} className="flex w-9 shrink-0 flex-col items-center gap-1 overflow-y-auto border-e border-border-variant py-2 [scrollbar-width:none]">
          {sections.map((section) => (
            <Tooltip key={section.id} interactive side="start" content={section.name} className="rounded-md">
              <button
                type="button"
                tabIndex={-1}
                data-rail={section.id}
                aria-label={section.name}
                aria-current={section.id === active || undefined}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => jump(section.id)}
                className={cn("relative grid size-6 place-items-center rounded-md outline-none hover:bg-hover-muted", section.id === active && "bg-hover-muted")}
              >
                {section.id === active && <span aria-hidden="true" className="absolute -start-1 h-3.5 w-[3px] rounded-e-full bg-text" />}
                {section.mark(14)}
              </button>
            </Tooltip>
          ))}
        </div>
      )}
      <div ref={listRef} onScroll={spy} className="model-menu-list relative min-h-0 flex-1 scroll-pt-7 overflow-x-hidden overflow-y-auto p-1">
        {searching ? (
          <>
            {found.filter((line) => !line.older).map((line) => row(line))}
            {foundOlder.lines.length > 0 && group(foundOlder)}
          </>
        ) : (
          sections.map(group)
        )}
        {unlisted > 0 && !searching && <div className="px-2 py-1.5 text-sm text-muted">{m.model_picker_more({ count: fmtNumber(unlisted) })}</div>}
        {searching && found.length === 0 && <div className="px-2 py-1.5 text-sm text-muted">{m.model_picker_no_matches({ query: filter.trim() })}</div>}
        {children}
      </div>
    </div>
  );
}

function ModelRow({ line, agents, selected, currentAgent, selectedAgent, lockedHarness, favorite, fresh, onPick, onToggleFavorite }: {
  line: PickerLine;
  agents: PickerAgent[];
  selected: boolean;
  currentAgent?: PickerAgent;
  selectedAgent?: PickerAgent;
  lockedHarness?: string;
  favorite: boolean;
  fresh: boolean;
  onPick: (line: PickerLine, agent: PickerAgent) => void;
  onToggleFavorite: (family: string, event: MouseEvent<HTMLElement>) => void;
}) {
  const free = line.offers[0].model.free;
  const name = lineName(line);
  const spoken = free ? `${name}, ${m.model_picker_free()}` : name;
  const auto = autoAgent(line, agents, currentAgent);
  const newChat = (agent: PickerAgent) => lockedHarness !== undefined && harnessKey(agent.harness) !== lockedHarness;
  // Arrow keys along a row move between its agents; Up and Down move between rows.
  const along = (event: KeyboardEvent<HTMLElement>) => {
    const rtl = getComputedStyle(event.currentTarget).direction === "rtl";
    const step = event.key === (rtl ? "ArrowLeft" : "ArrowRight") ? 1 : event.key === (rtl ? "ArrowRight" : "ArrowLeft") ? -1 : 0;
    if (!step) return;
    const stops = [...event.currentTarget.closest("[data-row]")!.querySelectorAll<HTMLElement>("[data-nav], [data-agent]")];
    const next = stops[stops.indexOf(event.currentTarget) + step];
    if (!next) return;
    event.preventDefault();
    next.focus();
  };
  return (
    <div data-row className={cn(ROW, selected && "bg-hover-muted")}>
      <button
        type="button"
        data-nav
        className={MAIN}
        aria-label={m.model_picker_model_on({ model: spoken, agent: auto.name })}
        aria-description={newChat(auto) ? m.model_picker_new_chat_with({ agent: auto.name }) : undefined}
        aria-pressed={selected}
        onKeyDown={along}
        onClick={() => onPick(line, auto)}
      >
        <ModelMark model={line.offers[0].model} harness={line.offers[0].tile.harness.id} size={16} />
        <span className="min-w-0 truncate">{name}</span>
        {free && <FreeBadge />}
        {fresh && <span className={cn(BADGE, "bg-accent-blue-subtle text-accent-blue-strong")}>{m.model_picker_new()}</span>}
        {selected && <Check size={14} className="shrink-0 text-accent-blue-strong" aria-hidden="true" />}
      </button>
      <span className="flex shrink-0 items-center">
        {agents.length > 1 && agents.map((agent) => (
          <Tooltip key={agent.key} interactive content={newChat(agent) ? m.model_picker_new_chat_with({ agent: agent.name }) : agent.name} className="rounded-md">
            <button
              type="button"
              tabIndex={-1}
              data-agent
              aria-label={m.model_picker_model_on({ model: spoken, agent: agent.name })}
              aria-description={newChat(agent) ? m.model_picker_new_chat_with({ agent: agent.name }) : undefined}
              aria-pressed={agent === selectedAgent}
              onKeyDown={along}
              onClick={() => onPick(line, agent)}
              className={cn(
                "grid h-7 w-6 place-items-center rounded-md outline-none transition-opacity focus-visible:outline-2 focus-visible:outline-focus",
                agent === selectedAgent ? "" : agent === auto ? "opacity-90 hover:opacity-100" : "opacity-50 hover:opacity-100",
              )}
            >
              <span className={cn("rounded-md", agent === selectedAgent && "ring-1 ring-accent-blue-strong")}>
                <AgentIconTile icon={agent.icon} brand={agent.brand} size="xs" />
              </span>
            </button>
          </Tooltip>
        ))}
      </span>
      <Tooltip interactive content={favorite ? m.model_picker_favorite_remove() : m.model_picker_favorite_add()} className="rounded-md">
        <button
          type="button"
          tabIndex={-1}
          data-agent
          aria-label={favorite ? m.model_picker_favorite_remove() : m.model_picker_favorite_add()}
          aria-pressed={favorite}
          onKeyDown={along}
          onClick={(event) => onToggleFavorite(line.family, event)}
          className="grid h-7 w-6 place-items-center rounded-md text-muted outline-none hover:text-text focus-visible:outline-2 focus-visible:outline-focus"
        >
          <Star size={14} aria-hidden="true" className={cn(favorite && "fill-current text-accent-amber")} />
        </button>
      </Tooltip>
    </div>
  );
}

export function FreeBadge() {
  return <span className={cn(BADGE, "bg-accent-green-subtle text-accent-green")}>{m.model_picker_free()}</span>;
}

export function offerOn(line: PickerLine | undefined, agent: PickerAgent | undefined, model: string | null): PickerOffer | undefined {
  return line?.offers.find((offer) => offer.tile === agent && offer.model.id === model);
}
