import { memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { defaultRangeExtractor } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Search, X } from "lucide-react";
import { fmtNumber, type ChatSearchHit, type ChatSearchScope, type Project } from "../api";
import { useWorkspace } from "../workspace";
import { chatSearchRanges, chatSearchSnippet } from "../chatSearch";
import { m } from "../paraglide/messages.js";
import { useLocale } from "../locale";
import { commandRegion } from "../keyboardRegion";
import { EscapeScope } from "./EscapeScope";
import { Button, IconButton, Spinner } from "./ui";

const RESULT_HEIGHT = 76;

const SearchSnippet = memo(function SearchSnippet({ hit }: { hit: ChatSearchHit }) {
  return <span className="chat-search-snippet">{chatSearchSnippet(hit.snippet)}</span>;
});

export const ChatSearch = memo(function ChatSearch({ projectId, sessionId, projects, assistantLabel, focusRequest, onOpen, onClose, branchReturn, pendingMessageId, embedded = false }: {
  projectId: string;
  sessionId: string | null;
  projects: Project[];
  assistantLabel: string;
  focusRequest: number;
  onOpen: (hit: ChatSearchHit, query: string) => void;
  onClose: () => void;
  branchReturn?: () => void;
  pendingMessageId?: string;
  embedded?: boolean;
}) {
  const locale = useLocale();
  const time = useMemo(() => new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }), [locale]);
  const [value, setValue] = useState(() => window.getSelection()?.toString().trim().slice(0, 256) ?? "");
  const [query, setQuery] = useState(value.trim());
  const [scope, setScope] = useState<ChatSearchScope>(sessionId ? "chat" : "project");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(true);
  const field = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const returnFocus = useRef(document.activeElement);
  const listId = useId();
  const statusId = useId();
  const highlightName = embedded ? "side-chat-search-results" : "chat-search-results";
  const currentScope = scope === "chat" && !sessionId ? "project" : scope;
  const params = { q: query, scope: currentScope, ...(currentScope === "chat" ? { sessionId: sessionId ?? undefined } : currentScope === "project" ? { projectId } : {}) };
  const { chatSearchQuery } = useWorkspace().queries;
  const options = chatSearchQuery(params);
  const requestKey = JSON.stringify([...options.queryKey, value.trim(), projectId, sessionId]);
  const liveRequest = useRef<string | null>(requestKey);
  liveRequest.current = requestKey;
  const results = useInfiniteQuery(options);
  const hits = useMemo(() => results.data?.pages.flatMap((page) => page.hits) ?? [], [results.data]);
  const selected = Math.max(0, hits.findIndex((hit) => hit.messageId === selectedId));
  const waiting = query !== value.trim() || results.isPlaceholderData;
  const page = results.data?.pages[0];
  const virtualizer = useVirtualizer({
    count: hits.length,
    getScrollElement: () => list.current,
    estimateSize: () => RESULT_HEIGHT,
    overscan: 3,
    rangeExtractor: (range) => [...new Set([...defaultRangeExtractor(range), ...(hits.length ? [selected] : [])])].sort((a, b) => a - b),
    useFlushSync: false,
  });
  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(value.trim()), 80);
    return () => window.clearTimeout(timer);
  }, [value]);
  useLayoutEffect(() => { setExpanded(true); field.current?.focus(); field.current?.select(); }, [focusRequest]);
  useLayoutEffect(() => { if (expanded) virtualizer.scrollToIndex(selected, { align: "auto" }); }, [expanded, selected, virtualizer]);
  useLayoutEffect(() => () => {
    liveRequest.current = null;
    const target = returnFocus.current;
    if (target instanceof HTMLElement && target.isConnected) target.focus({ preventScroll: true });
  }, []);

  // Enter pressed while results catch up with typing opens the first one when it arrives.
  const queuedOpen = useRef(false);
  const settling = waiting || Boolean(query) && results.isPending;
  const open = (index: number) => {
    if (settling) { queuedOpen.current = true; return; }
    const hit = hits[index];
    if (!hit) return;
    setSelectedId(hit.messageId);
    setExpanded(false);
    onOpen(hit, query);
  };
  useEffect(() => {
    if (settling || !queuedOpen.current) return;
    queuedOpen.current = false;
    open(selected);
  }, [settling]);
  const move = async (direction: number, navigate: boolean) => {
    if (waiting || results.isFetchingNextPage || hits.length === 0) return;
    if (direction < 0 && selected === 0 && results.hasNextPage) return;
    const next = selected + direction;
    if (next >= hits.length && results.hasNextPage) {
      const loaded = await results.fetchNextPage({ cancelRefetch: false });
      if (liveRequest.current !== requestKey) return;
      const updated = loaded.data?.pages.flatMap((page) => page.hits) ?? [];
      const hit = updated[next] ?? (!loaded.hasNextPage ? updated[0] : undefined);
      if (hit) { setSelectedId(hit.messageId); if (navigate) { setExpanded(false); onOpen(hit, query); } }
      return;
    }
    const index = (next + hits.length) % hits.length;
    setSelectedId(hits[index].messageId);
    if (navigate) { setExpanded(false); onOpen(hits[index], query); }
  };
  useEffect(() => {
    const navigate = (event: KeyboardEvent) => {
      const input = field.current;
      if (event.defaultPrevented || event.isComposing || event.altKey || !input || input.closest("[inert]")) return;
      const modified = event.metaKey !== event.ctrlKey;
      const find = event.key.toLowerCase() === "f" && modified && !event.shiftKey;
      const next = event.key === "F3" && !event.metaKey && !event.ctrlKey
        || event.key.toLowerCase() === "g" && modified;
      if (!find && !next) return;
      const right = commandRegion(event.target) === "right";
      if (embedded ? !right : right && document.querySelector(".right-pane [role=search]")) return;
      event.preventDefault();
      event.stopPropagation();
      if (find) {
        setExpanded(true);
        input.focus();
        input.select();
        return;
      }
      if (expanded && !event.shiftKey) open(selected);
      else void move(event.shiftKey ? -1 : 1, true);
    };
    document.addEventListener("keydown", navigate, true);
    return () => document.removeEventListener("keydown", navigate, true);
  }, [embedded, expanded, selected, open, move]);
  const status = !expanded && pendingMessageId && hits[selected]?.messageId === pendingMessageId ? m.chat_search_waiting_for_turn()
    : waiting || results.isPending && query ? m.chat_search_searching()
    : results.isError ? m.chat_search_failed()
    : page?.requiresLongerQuery ? m.chat_search_short_query()
    : !query ? m.chat_search_hint()
    : hits.length === 0 ? m.chat_search_empty()
    : m.chat_search_position({ position: fmtNumber(selected + 1), count: `${fmtNumber(hits.length)}${results.hasNextPage ? "+" : ""}` });
  const statusInField = hits.length > 0 && !waiting && !results.isError && hits[selected]?.messageId !== pendingMessageId;
  const virtualItems = virtualizer.getVirtualItems();
  useLayoutEffect(() => {
    if (!("highlights" in CSS) || typeof Highlight === "undefined") return;
    const ranges = [...(list.current?.querySelectorAll<HTMLElement>(".chat-search-snippet") ?? [])].flatMap((root) => chatSearchRanges(root, query));
    const highlight = new Highlight(...ranges);
    CSS.highlights.set(highlightName, highlight);
    return () => { if (CSS.highlights.get(highlightName) === highlight) CSS.highlights.delete(highlightName); };
  }, [query, hits, virtualItems, highlightName]);

  return <EscapeScope onEscape={onClose} exclusive={false}>
    <div role="search" aria-label={m.chat_search_title()} className={`chat-search relative z-30 ms-auto mb-2 w-full rounded-lg border border-border bg-background shadow-popover ${expanded ? "max-w-lg" : "max-w-sm"}`}>
      <div className="flex items-center gap-1 px-2 py-1.5">
        <Search size={16} className="mx-1 shrink-0 text-muted" aria-hidden="true" />
        <input ref={field} type="text" role="combobox" aria-label={m.chat_search_title()} aria-keyshortcuts="Control+f Meta+f" aria-autocomplete="list" aria-expanded={expanded && hits.length > 0} aria-controls={expanded && hits.length ? listId : undefined} aria-activedescendant={expanded && hits[selected] ? `${listId}-${selected}` : undefined} aria-describedby={statusId}
          maxLength={256} autoComplete="off" spellCheck={false} value={value} placeholder={m.chat_search_placeholder()}
          className="h-8 w-full min-w-0 border-0 bg-transparent px-1 text-base text-text outline-none"
          onChange={(event) => { queuedOpen.current = false; setValue(event.target.value); setSelectedId(null); setExpanded(true); }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "Enter") { event.preventDefault(); if (expanded && !event.shiftKey) open(selected); else void move(event.shiftKey ? -1 : 1, true); }
            else if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); void move(event.key === "ArrowDown" ? 1 : -1, !expanded); }
          }} />
        <span className="flex w-4 shrink-0 items-center">{results.isFetching && <Spinner />}</span>
        <span id={statusId} role="status" aria-live="polite" aria-atomic="true" className={statusInField ? "shrink-0 whitespace-nowrap px-1 text-xs tabular-nums text-subtext" : "sr-only"}>{status}</span>
        <IconButton size="small" aria-label={m.chat_search_previous()} aria-keyshortcuts="Shift+F3 Control+Shift+g Meta+Shift+g" data-tip={m.chat_search_previous()} disabled={!hits.length || waiting || results.isFetchingNextPage || selected === 0 && results.hasNextPage} onClick={() => void move(-1, true)}><ArrowUp size={15} /></IconButton>
        <IconButton size="small" aria-label={m.chat_search_next()} aria-keyshortcuts="F3 Control+g Meta+g" data-tip={m.chat_search_next()} disabled={!hits.length || waiting || results.isFetchingNextPage} onClick={() => { if (expanded) open(selected); else void move(1, true); }}><ArrowDown size={15} /></IconButton>
        <IconButton size="small" aria-label={m.chat_search_close()} data-tip={m.chat_search_close()} onClick={onClose}><X size={15} /></IconButton>
      </div>
      {(expanded || !statusInField || page?.indexing || branchReturn) && <div className="absolute inset-x-0 top-full mt-1 overflow-hidden rounded-lg border border-border bg-background shadow-popover">
        {(expanded || !statusInField || branchReturn) && <div className="flex flex-wrap items-center gap-1 px-2 py-1.5">
          {expanded && <div role="group" aria-label={m.chat_search_scope()} className="flex gap-0.5">
            {([ ["chat", m.chat_search_chat()], ["project", m.chat_search_project()], ["all", m.chat_search_all()] ] as const).map(([id, label]) => <button key={id} type="button" aria-pressed={currentScope === id} disabled={id === "chat" && !sessionId}
              className={`min-h-7 cursor-pointer rounded-sm px-2 text-sm focus-visible:outline-2 focus-visible:outline-focus disabled:opacity-45 ${currentScope === id ? "bg-surface text-text" : "text-subtext hover:bg-surface"}`}
              onClick={() => { setScope(id); setSelectedId(null); setExpanded(true); field.current?.focus(); }}>{label}</button>)}
          </div>}
          {!statusInField && <span aria-hidden="true" className="ms-auto px-1 text-xs text-subtext">{status}</span>}
          {branchReturn && <Button className="ms-auto" size="small" onClick={branchReturn}>{m.chat_search_previous_branch()}</Button>}
        </div>}
        {page?.indexing && <div className="px-3 pb-2 text-xs text-subtext" role="status">{m.chat_search_indexing()}</div>}
        {results.isError && <div className="flex justify-end px-3 pb-2"><Button size="small" onClick={() => void results.refetch()}>{m.app_retry()}</Button></div>}
        {expanded && hits.length > 0 && <>
          <div ref={list} id={listId} role="listbox" aria-label={m.chat_search_results()} aria-busy={waiting || results.isFetching} className={`overflow-y-auto overscroll-contain border-t border-border ${waiting ? "opacity-60" : ""}`} style={{ height: Math.min(hits.length * RESULT_HEIGHT, 320) }}>
            <div className="relative" style={{ height: virtualizer.getTotalSize() }}>
              {virtualItems.map((row) => {
                const hit = hits[row.index];
                const project = projects.find((project) => project.id === hit.projectId);
                return <div key={hit.messageId} id={`${listId}-${row.index}`} role="option" aria-selected={selected === row.index} aria-posinset={row.index + 1} aria-setsize={results.hasNextPage ? -1 : hits.length}
                  className={`absolute inset-x-0 cursor-pointer px-3 py-2 ${selected === row.index ? "bg-surface" : "hover:bg-panel"}`} style={{ top: row.start, height: row.size }}
                  onPointerDown={(event) => event.preventDefault()} onClick={() => open(row.index)}>
                  <div className="flex h-5 items-center gap-2 overflow-hidden text-xs text-subtext"><span className="min-w-0 flex-1 truncate">{currentScope !== "chat" ? hit.chatTitle || m.chat_untitled() : hit.role === "user" ? m.chat_export_you() : assistantLabel}</span>
                    {currentScope === "all" && project && <span className="max-w-28 truncate">{project.name}</span>}
                    {hit.archived && <span>{m.chat_panel_archived()}</span>}{!hit.onActiveBranch && <span>{m.chat_search_other_branch()}</span>}
                    <time className="shrink-0 text-muted" dateTime={new Date(hit.createdAt).toISOString()}>{time.format(hit.createdAt)}</time>
                  </div>
                  <div dir="auto" className="line-clamp-2 text-sm leading-5 text-text"><SearchSnippet hit={hit} /></div>
                </div>;
              })}
            </div>
          </div>
          {results.hasNextPage && <Button className="w-full rounded-none rounded-b-lg" size="small" disabled={results.isFetchingNextPage} onClick={() => void results.fetchNextPage()}>{m.chat_search_more()}</Button>}
        </>}
      </div>}
    </div>
  </EscapeScope>;
});
