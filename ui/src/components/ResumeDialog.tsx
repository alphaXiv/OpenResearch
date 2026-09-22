import { useQuery } from "@tanstack/react-query";
import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { timeAgo, type ChatSession, type NativeChat } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { listAllChatSessionsQuery, listNativeChatsQuery } from "../queries/chat";
import { listProjectsQuery } from "../queries/projects";
import { HarnessLogo } from "./HarnessLogo";
import { HARNESS_LABELS } from "./ModelPicker";
import { Input, LoadingRow, MenuItem, Spinner } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

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
  onClose,
  onResume,
  onImport,
}: {
  activeSessionId: string | null;
  onClose: () => void;
  onResume: (session: ChatSession) => void;
  onImport: (chat: NativeChat) => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);
  const { data: sessions, isPending, error } = useQuery(listAllChatSessionsQuery());
  const { data: projects = [] } = useQuery(listProjectsQuery());
  const { data: nativeChats = [] } = useQuery(listNativeChatsQuery());
  const [filter, setFilter] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  useDialogFocus(dialogRef, onClose);

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
        hit(`${session.title ?? ""} ${projectNames.get(session.projectId) ?? ""} ${HARNESS_LABELS[session.harness]}`))
      .map((session) => ({ kind: "session", key: session.id, session }));
    const native: ResumeEntry[] = nativeChats
      .filter((chat) => hit(`${chat.title ?? ""} ${chat.cwd ?? ""} ${HARNESS_LABELS[chat.harness]}`))
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

  const pick = (entry: ResumeEntry) =>
    entry.kind === "native" ? onImport(entry.chat) : onResume(entry.session);

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-start justify-center bg-modal-backdrop p-5 pt-[var(--modal-top)]"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="flex max-h-[calc(100vh_-_var(--modal-top)_-_1.25rem)] w-140 max-w-full flex-col rounded-xl border border-border bg-background shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-label={m.resume_dialog_title()}
        tabIndex={-1}
      >
        <div className="border-b border-border p-3">
          <Input
            data-initial-focus
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
      </div>
    </div>,
    document.body,
  );
}
