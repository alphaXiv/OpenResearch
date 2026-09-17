import { useQuery } from "@tanstack/react-query";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { timeAgo, type ChatSession } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { listAllChatSessionsQuery } from "../queries/chat";
import { listProjectsQuery } from "../queries/projects";
import { HarnessLogo } from "./HarnessLogo";
import { HARNESS_LABELS } from "./ModelPicker";
import { Input, LoadingRow, MenuItem, Spinner } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

/** The composer's `/resume` picker: every chat in every project, newest first. */
export function ResumeDialog({
  activeSessionId,
  onClose,
  onResume,
}: {
  activeSessionId: string | null;
  onClose: () => void;
  onResume: (session: ChatSession) => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);
  const { data: sessions, isPending, error } = useQuery(listAllChatSessionsQuery());
  const { data: projects = [] } = useQuery(listProjectsQuery());
  const [filter, setFilter] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  useDialogFocus(dialogRef, onClose);

  const projectNames = useMemo(
    () => new Map(projects.map((project) => [project.id, project.name])),
    [projects],
  );
  const matches = useMemo(() => {
    const query = filter.trim().toLowerCase();
    return (sessions ?? []).filter((session) => {
      if (session.id === activeSessionId) return false;
      const haystack = `${session.title ?? ""} ${projectNames.get(session.projectId) ?? ""} ${HARNESS_LABELS[session.harness]}`;
      return !query || haystack.toLowerCase().includes(query);
    });
  }, [sessions, filter, activeSessionId, projectNames]);
  const selected = Math.min(activeIndex, Math.max(0, matches.length - 1));

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [selected, matches]);

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
            aria-activedescendant={matches[selected] ? `resume-option-${matches[selected].id}` : undefined}
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
                onResume(matches[selected]);
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
            matches.map((session, index) => (
              <MenuItem
                key={session.id}
                id={`resume-option-${session.id}`}
                ref={index === selected ? activeRef : undefined}
                type="button"
                role="option"
                aria-selected={index === selected}
                active={index === selected}
                tabIndex={-1}
                className="gap-2.5 text-text"
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => onResume(session)}
              >
                <HarnessLogo harness={session.harness} />
                <span dir="auto" className="min-w-0 flex-1 truncate">{session.title?.trim() || m.chat_untitled()}</span>
                <span dir="auto" className="max-w-40 shrink-0 truncate text-muted">
                  {projectNames.get(session.projectId) ?? ""}
                </span>
                <span className="shrink-0 text-muted tabular-nums">{timeAgo(session.updatedAt)}</span>
              </MenuItem>
            ))
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
