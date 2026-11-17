import { ArrowRight, ChevronLeft, ChevronRight, CircleHelp, Pencil, X } from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";
import type { ChatPart, PromptAnswer } from "../api";
import { m } from "../paraglide/messages.js";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { Button, IconButton } from "./ui";

interface Answer {
  picked: string[];
  note: string;
}

const UNANSWERED: Answer = { picked: [], note: "" };

/** Long enough to see the choice land before the next question replaces it. */
const ADVANCE_DELAY_MS = 200;

/**
 * The questions chat `scope` is waiting on, answered one at a time and
 * delivered together, so the user can go back until the last one is submitted.
 * A question left without a choice or a note is delivered as skipped.
 */
export function useQuestions(scope: string | null, pending: ChatPart[], respond: (answer: PromptAnswer) => Promise<boolean>) {
  const [answers, setAnswers] = useState<Record<string, Answer>>({});
  const [currentId, setCurrentId] = useState<string | null>(null);
  /** Delivered questions leave at once; the transcript catches up later. */
  const [delivered, setDelivered] = useState<ReadonlySet<string>>(new Set());
  /** The questions being delivered, held still while their answers go out. */
  const [delivering, setDelivering] = useState<ChatPart[] | null>(null);
  const advanceTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(advanceTimer.current), [scope]);

  const [shownScope, setShownScope] = useState(scope);
  if (shownScope !== scope) {
    setShownScope(scope);
    setAnswers({});
    setCurrentId(null);
    setDelivering(null);
  }

  const open = delivering ?? pending.filter((part) => !delivered.has(part.id));
  const index = Math.max(0, open.findIndex((part) => part.id === currentId));
  const part = open.at(index) ?? null;
  const answer = (part && answers[part.id]) ?? UNANSWERED;
  const answered = answer.picked.length > 0 || answer.note.trim() !== "";

  /** Read by the delayed advance, which must not send an answer older than the screen's. */
  const latestAnswers = useRef(answers);
  latestAnswers.current = answers;
  const edit = (next: Answer) => {
    if (!part) return;
    latestAnswers.current = { ...latestAnswers.current, [part.id]: next };
    setAnswers(latestAnswers.current);
  };

  const deliver = async () => {
    // Leaving the chat clears its answers; the ones on their way still go out.
    const all = latestAnswers.current;
    setDelivering(open);
    try {
      for (const question of open) {
        const { picked, note } = all[question.id] ?? UNANSWERED;
        const text = note.trim();
        const sent = await respond(
          picked.length > 0 || text
            ? { promptId: question.id, answers: picked, note: text || undefined }
            : { promptId: question.id, approve: false },
        );
        if (!sent) {
          setCurrentId(question.id);
          return;
        }
        setDelivered((ids) => new Set(ids).add(question.id));
      }
    } finally {
      setDelivering(null);
    }
  };

  const advance = (from: ChatPart) => {
    window.clearTimeout(advanceTimer.current);
    const next = open.at(open.indexOf(from) + 1);
    if (next) setCurrentId(next.id);
    else void deliver();
  };

  return {
    part,
    index,
    total: open.length,
    last: index === open.length - 1,
    answer,
    answered,
    busy: delivering !== null,
    show: setCurrentId,
    setNote: (note: string) => edit({ ...answer, note }),
    /** One choice answers a single-select question; a multi-select one waits for Next. */
    pick: (label: string) => {
      if (!part || delivering) return;
      if (part.prompt?.multiSelect) {
        const picked = answer.picked.includes(label) ? answer.picked.filter((item) => item !== label) : [...answer.picked, label];
        edit({ ...answer, picked });
        return;
      }
      edit({ ...answer, picked: [label] });
      window.clearTimeout(advanceTimer.current);
      advanceTimer.current = window.setTimeout(() => advance(part), ADVANCE_DELAY_MS);
    },
    next: () => {
      if (part && !delivering && answered) advance(part);
    },
    skip: () => {
      if (!part || delivering) return;
      edit(UNANSWERED);
      advance(part);
    },
    back: () => {
      window.clearTimeout(advanceTimer.current);
      if (!delivering && index > 0) setCurrentId(open[index - 1].id);
    },
    forward: () => {
      window.clearTimeout(advanceTimer.current);
      if (!delivering && index < open.length - 1) setCurrentId(open[index + 1].id);
    },
    /** Delivers every open question as skipped. */
    dismiss: () => {
      if (delivering) return;
      window.clearTimeout(advanceTimer.current);
      latestAnswers.current = {};
      setAnswers({});
      void deliver();
    },
  };
}

type Questions = ReturnType<typeof useQuestions>;

/** The questions a chat is waiting on, answered in a card above the composer. */
export function QuestionDock({
  questions,
  disabled,
  hotkeys,
  searchTarget,
}: {
  questions: Questions & { part: ChatPart };
  disabled: boolean;
  /** Whether 1–9 pick an option while focus is outside a text field. */
  hotkeys: boolean;
  searchTarget: boolean;
}) {
  const { id: elementId } = useContext(WorkspaceDomContext);
  const root = useRef<HTMLDivElement>(null);
  const { part, index, total, answer, busy } = questions;
  const prompt = part.prompt;
  const options = prompt?.options ?? [];
  const questionId = elementId(`question-${part.id}`);
  const inactive = disabled || busy;
  const placeholder = answer.picked.length > 0
    ? m.chat_question_add_note()
    : options.length > 0 ? m.chat_question_custom_response() : m.chat_question_your_answer();

  const latest = useRef({ options, pick: questions.pick });
  latest.current = { options, pick: questions.pick };
  useEffect(() => {
    if (!hotkeys || inactive) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (!(target instanceof HTMLElement) || target.isContentEditable || target.matches("input, textarea, select")) return;
      if (target !== document.body && !root.current?.closest(".chat-pane")?.contains(target)) return;
      const option = /^[1-9]$/.test(event.key) ? latest.current.options[Number(event.key) - 1] : undefined;
      if (!option) return;
      event.preventDefault();
      latest.current.pick(option.label);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [hotkeys, inactive]);

  return (
    <div ref={root} className="chat-prompt-questions mb-2 min-h-0 overflow-y-auto rounded-xl border border-border bg-background shadow-control-subtle">
      <div
        className="flex flex-col p-1.5 outline-none"
        role="group"
        aria-labelledby={questionId}
        data-part-id={part.id}
        data-search-target={searchTarget || undefined}
        tabIndex={-1}
      >
        <div className="flex items-center gap-1.5 ps-2.5 text-sm text-subtext">
          <CircleHelp size={14} className="shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1 truncate">{prompt?.header || m.chat_question()}</span>
          {total > 1 && (
            <>
              <IconButton type="button" size="small" aria-label={m.chat_question_back()} disabled={inactive || index === 0} onClick={questions.back}>
                <ChevronLeft size={14} className="rtl:rotate-180" />
              </IconButton>
              <span className="text-xs tabular-nums" aria-label={m.chat_question_progress({ current: index + 1, total })}>
                {m.chat_question_position({ current: index + 1, total })}
              </span>
              <IconButton type="button" size="small" aria-label={m.chat_question_forward()} disabled={inactive || questions.last} onClick={questions.forward}>
                <ChevronRight size={14} className="rtl:rotate-180" />
              </IconButton>
            </>
          )}
          <IconButton type="button" size="small" aria-label={m.chat_question_dismiss()} disabled={inactive} onClick={questions.dismiss}>
            <X size={14} />
          </IconButton>
        </div>
        <div id={questionId} className="mt-1 px-2.5 text-sm font-medium leading-snug text-text whitespace-pre-line wrap-anywhere">
          {prompt?.question?.replace(/\n\s*\n/g, "\n")}
        </div>
        {prompt?.multiSelect && <div className="px-2.5 pt-0.5 text-sm text-subtext">{m.chat_question_choose_any()}</div>}
        {options.length > 0 && (
          <div className="mt-1.5 flex flex-col">
            {options.map((option, optionIndex) => {
              const selected = answer.picked.includes(option.label);
              return (
                <button
                  type="button"
                  key={option.label}
                  aria-pressed={selected}
                  disabled={inactive}
                  className={`group flex w-full items-start gap-2.5 rounded-md px-2.5 py-1.5 text-start text-sm text-text cursor-pointer transition-colors duration-120 ease-standard focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-solid focus-visible:outline-focus disabled:cursor-default ${selected ? "bg-surface" : "enabled:hover:bg-surface"}`}
                  onClick={() => questions.pick(option.label)}
                >
                  <span
                    className={`flex size-5 shrink-0 items-center justify-center rounded-xs border text-xs tabular-nums ${selected ? "border-transparent bg-text text-background" : "border-border text-subtext"}`}
                    aria-hidden="true"
                  >
                    {optionIndex < 9 ? optionIndex + 1 : null}
                  </span>
                  <span className="min-w-0 flex-1 pt-px">
                    <span className="font-medium">{option.label}</span>
                    {option.description && option.description !== option.label && <> — {option.description}</>}
                  </span>
                  <ArrowRight size={14} className={`mt-0.5 shrink-0 text-subtext rtl:rotate-180 ${selected ? "" : "opacity-0 group-enabled:group-hover:opacity-100"}`} aria-hidden="true" />
                </button>
              );
            })}
          </div>
        )}
        <div className="mt-1 flex items-center gap-2 ps-2.5">
          <Pencil size={14} className="shrink-0 text-subtext" aria-hidden="true" />
          <input
            className="min-w-0 flex-1 rounded-none border-0 bg-transparent px-0 py-1.5 text-sm text-text outline-none placeholder:text-subtext"
            value={answer.note}
            placeholder={placeholder}
            aria-label={placeholder}
            readOnly={inactive}
            onChange={(event) => questions.setNote(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Enter" || event.nativeEvent.isComposing) return;
              event.preventDefault();
              questions.next();
            }}
          />
          <Button type="button" size="small" disabled={inactive} onClick={questions.skip}>
            {m.chat_question_skip()}
          </Button>
          <Button type="button" size="small" variant="primary" disabled={inactive || !questions.answered} onClick={questions.next}>
            {questions.last ? m.chat_panel_submit() : m.chat_question_next()}
          </Button>
        </div>
      </div>
    </div>
  );
}
