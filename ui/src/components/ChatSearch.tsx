import { ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { scrollTranscriptSearchRangeIntoView, transcriptSearchRanges } from "../chatSearch";
import { fmtNumber } from "../api";
import { m } from "../paraglide/messages.js";
import { IconButton } from "./ui";

export function ChatSearch({ rootRef, scrollRef, inputRef, embedded, onClose, onNavigate }: {
  rootRef: RefObject<HTMLDivElement | null>;
  scrollRef: RefObject<HTMLDivElement | null>;
  inputRef: RefObject<HTMLInputElement | null>;
  embedded: boolean;
  onClose: () => void;
  onNavigate: () => void;
}) {
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<Range[]>([]);
  const [index, setIndex] = useState(0);
  const [navigation, setNavigation] = useState(0);
  const selected = useRef<Range | null>(null);
  const lastScrolled = useRef<{ node: Node; offset: number; query: string } | null>(null);
  const highlightName = embedded ? "side-chat-find" : "chat-find";

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    let frame = 0;
    const update = () => {
      const ranges = transcriptSearchRanges(root, query);
      const previous = selected.current;
      const retained = previous ? ranges.findIndex((range) =>
        range.startContainer === previous.startContainer && range.startOffset === previous.startOffset) : -1;
      setMatches(ranges);
      setIndex((current) => retained >= 0 ? retained : Math.min(current, Math.max(0, ranges.length - 1)));
    };
    update();
    // Streamed text, expanded work, and edits can change the searchable DOM.
    const observer = new MutationObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(update);
    });
    observer.observe(root, {
      childList: true, characterData: true, subtree: true,
      attributes: true, attributeFilter: ["aria-hidden", "hidden", "inert", "open"],
    });
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [query, rootRef]);

  useLayoutEffect(() => {
    if (!("highlights" in CSS) || typeof Highlight === "undefined") return;
    const all = new Highlight();
    for (const range of matches) all.add(range);
    const active = new Highlight(...(matches[index] ? [matches[index]] : []));
    active.priority = 1;
    CSS.highlights.set(highlightName, all);
    CSS.highlights.set(`${highlightName}-active`, active);
    return () => {
      CSS.highlights.delete(highlightName);
      CSS.highlights.delete(`${highlightName}-active`);
    };
  }, [matches, index, highlightName]);

  useEffect(() => {
    const range = matches[index];
    selected.current = range ?? null;
    if (!range) return;
    const previous = lastScrolled.current;
    if (previous?.node === range.startContainer && previous.offset === range.startOffset && previous.query === query) return;
    onNavigate();
    // Let the virtualizer measure the newly mounted history before scrolling.
    const frame = requestAnimationFrame(() => {
      const scroll = scrollRef.current;
      if (!scroll || !range.startContainer.isConnected) return;
      scrollTranscriptSearchRangeIntoView(range, scroll);
      lastScrolled.current = { node: range.startContainer, offset: range.startOffset, query };
    });
    return () => cancelAnimationFrame(frame);
  }, [matches, index, navigation, query, scrollRef, onNavigate]);

  const navigate = (direction: number) => {
    if (!matches.length) return;
    lastScrolled.current = null;
    setIndex((current) => (current + direction + matches.length) % matches.length);
    setNavigation((current) => current + 1);
  };

  return (
    <div role="search" aria-label={m.chat_find()} className="relative z-5 mx-auto flex w-full max-w-readable shrink-0 items-center gap-1 border-b border-border bg-background px-4 py-2">
      <Search size={15} className="shrink-0 text-subtext" />
      <input
        ref={inputRef}
        autoFocus
        type="search"
        className="min-w-0 flex-1 border-0 bg-transparent text-sm outline-none"
        aria-label={m.chat_find()}
        placeholder={m.chat_find()}
        value={query}
        onChange={(event) => {
          selected.current = null;
          lastScrolled.current = null;
          setIndex(0);
          setQuery(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
          if (event.key === "Enter") {
            event.preventDefault();
            navigate(event.shiftKey ? -1 : 1);
          }
        }}
      />
      <span role="status" aria-live="polite" aria-atomic="true" className="shrink-0 text-xs tabular-nums text-subtext">
        {query ? matches.length ? m.chat_find_count({ current: fmtNumber(index + 1), total: fmtNumber(matches.length) }) : m.chat_find_no_matches() : ""}
      </span>
      <IconButton size="small" disabled={!matches.length} title={m.chat_find_previous()} aria-label={m.chat_find_previous()} onClick={() => navigate(-1)}>
        <ChevronUp size={15} />
      </IconButton>
      <IconButton size="small" disabled={!matches.length} title={m.chat_find_next()} aria-label={m.chat_find_next()} onClick={() => navigate(1)}>
        <ChevronDown size={15} />
      </IconButton>
      <IconButton size="small" title={m.chat_find_close()} aria-label={m.chat_find_close()} onClick={onClose}>
        <X size={15} />
      </IconButton>
    </div>
  );
}
