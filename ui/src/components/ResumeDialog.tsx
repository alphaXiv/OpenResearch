import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { timeAgo, type ChatSession, type NativeChat } from "../api";
import { type DialogContentProps } from "../dialog";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { listAllChatSessionsQuery, listNativeChatsQuery } from "../queries/chat";
import { listProjectsQuery } from "../queries/projects";
import { HarnessLogo } from "./HarnessLogo";
import { harnessLabel } from "./ModelPicker";
import { DialogContent, Input, LoadingRow, MenuItem, Spinner } from "./ui";

/** The composer's `/resume` picker: every chat in every project, newest first. */
/** The agents' own chats follow orx's, all under one labelled group so a
 * screen reader announces the heading once for the whole section. */
function renderRows(
  matches: ResumeEntry[],
  firstNative: number,
  row: (entry: ResumeEntry, index: number) => ReactNode,
) {
  const rows = matches.map(row);
  if (firstNative < 0) return rows;
  return [
    ...rows.slice(0, firstNative),
    <div key="from-terminal" role="group" aria-label={m.resume_dialog_from_terminal()}>
      <div className="px-2.5 pb-1 pt-2.5 text-sm font-medium text-subtext">
        {m.resume_dialog_from_terminal()}
      </div>
      {rows.slice(firstNative)}
    </div>,
  ];
}

/** One row: a chat orx already has, or one still in an agent's own store. */
type ResumeEntry =
  | { kind: "session"; key: string; session: ChatSession }
  | { kind: "native"; key: string; chat: NativeChat };

/** The composer's `/resume` picker: every chat in every project, newest first,
 * plus the ones still in an agent's own CLI. */
export function ResumeDialog({
  activeSessionId,
  cancel,
  onResume,
  onImport,
  cancelRef,
}: DialogContentProps<void> & {
  activeSessionId: string | null;
  onResume: (session: ChatSession) => void;
  onImport: (chat: NativeChat) => void;
  cancelRef: RefObject<(() => void) | null>;
}) {
  useEffect(() => {
    cancelRef.current = cancel;
    return () => { if (cancelRef.current === cancel) cancelRef.current = null; };
  }, [cancelRef, cancel]);

  const searchRef = useRef<HTMLInputElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);
  const { data: sessions, isPending, error } = useQuery(listAllChatSessionsQuery());
  const { data: projects = [] } = useQuery(listProjectsQuery());
  const { data: nativeChats = [] } = useQuery(listNativeChatsQuery());
  const [filter, setFilter] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);

  const projectNames = useMemo(
    () => new Map(projects.map((project) => [project.id, project.name])),
    [projects],
  );
  const matches = useMemo<ResumeEntry[]>(() => {
    const query = filter.trim().toLowerCase();
    const hit = (haystack: string) => !query || haystack.toLowerCase().includes(query);
    const own: ResumeEntry[] = (sessions ?? [])
      .filter((session) => session.id !== activeSessionId)
      .filter((session) =>
        hit(`${session.title ?? ""} ${projectNames.get(session.projectId) ?? ""} ${harnessLabel(session.harness, session.harnessName)}`))
      .map((session) => ({ kind: "session", key: session.id, session }));
    const native: ResumeEntry[] = nativeChats
      .filter((chat) => hit(`${chat.title ?? ""} ${chat.cwd ?? ""} ${harnessLabel(chat.harness)}`))
      .map((chat) => ({ kind: "native", key: `${chat.harness}:${chat.nativeId}`, chat }));
    return [...own, ...native];
  }, [sessions, nativeChats, filter, activeSessionId, projectNames]);
  const firstNative = useMemo(() => matches.findIndex((entry) => entry.kind === "native"), [matches]);
  const selected = Math.min(activeIndex, Math.max(0, matches.length - 1));

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [selected, matches]);

  /** The tail of a path distinguishes chats; the shared prefix does not. */
  const folderName = (cwd: string | null) => cwd?.split("/").filter(Boolean).at(-1) ?? "";

  const pick = (entry: ResumeEntry) => {
    cancel();
    if (entry.kind === "native") onImport(entry.chat);
    else onResume(entry.session);
  };

  return <DialogContent
      initialFocus={searchRef}
      className="flex max-h-[calc(100vh-2.5rem)] w-140 flex-col p-0"
      aria-label={m.resume_dialog_title()}
    >
      <div className="border-b border-border p-3">
        <Input
          ref={searchRef}
          value={filter}
          placeholder={m.resume_dialog_search()}
          aria-label={m.resume_dialog_search()}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={matches.length > 0}
          aria-controls="resume-options"
          aria-activedescendant={matches[selected] ? `resume-option-${matches[selected].key}` : undefined}
          onChange={(event) => {
            setFilter(event.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={(event) => {
            // Enter and the arrows belong to the IME while a candidate is open.
            if (event.nativeEvent.isComposing) return;
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              if (matches.length === 0) return;
              const delta = event.key === "ArrowDown" ? 1 : -1;
              setActiveIndex((selected + delta + matches.length) % matches.length);
            } else if (event.key === "Enter" && matches[selected]) {
              event.preventDefault();
              pick(matches[selected]);
            }
          }}
        />
      </div>
      <div
        id="resume-options"
        className="min-h-0 flex-1 overflow-y-auto p-1.5"
        role={matches.length > 0 ? "listbox" : undefined}
        aria-label={matches.length > 0 ? m.resume_dialog_title() : undefined}
      >
        {isPending ? (
          <LoadingRow className="px-2.5 py-2" role="status">
            <Spinner /> {m.common_loading()}
          </LoadingRow>
        ) : error && !sessions ? (
          <div className="px-2.5 py-2 text-sm text-accent-red" role="alert">
            {m.common_failed_to_load({ error: ltr(error instanceof Error ? error.message : String(error)) })}
          </div>
        ) : matches.length === 0 ? (
          <div className="px-2.5 py-2 text-sm text-muted" role="status">{m.resume_dialog_empty()}</div>
        ) : (
          renderRows(matches, firstNative, (entry, index) => {
            const native = entry.kind === "native" ? entry.chat : null;
            const session = entry.kind === "session" ? entry.session : null;
            const row = (
              <MenuItem
                key={entry.key}
                id={`resume-option-${entry.key}`}
                ref={index === selected ? activeRef : undefined}
                type="button"
                role="option"
                aria-selected={index === selected}
                active={index === selected}
                tabIndex={-1}
                className="gap-2.5 text-text"
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => pick(entry)}
              >
                <HarnessLogo harness={native?.harness ?? session?.harness ?? "claude-code"} />
                <span dir="auto" className="min-w-0 flex-1 truncate">
                  {(native?.title ?? session?.title)?.trim() || m.chat_untitled()}
                </span>
                <span
                  dir="auto"
                  className="max-w-40 shrink-0 truncate text-muted"
                  title={native?.cwd ?? undefined}
                >
                  {native ? folderName(native.cwd) : projectNames.get(session?.projectId ?? "") ?? ""}
                </span>
                <span className="shrink-0 text-muted tabular-nums">
                  {timeAgo(native?.updatedAt ?? session?.updatedAt ?? 0)}
                </span>
              </MenuItem>
            );
            return row;
          })
        )}
      </div>
    </DialogContent>;
}
