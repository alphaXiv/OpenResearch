import { Check } from "lucide-react";
import { useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ChatPart, PromptAnswer } from "../api";
import { m } from "../paraglide/messages.js";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { ResizeHandle } from "./ResizeHandle";

interface Answer {
  picked: string[];
  note: string;
}

const UNANSWERED: Answer = { picked: [], note: "" };

/** Long enough to see the choice land before the next question replaces it. */
const ADVANCE_DELAY_MS = 200;

/** The least the question shows once the options and the answer field have claimed their space. */
const MIN_HEIGHT = 48;

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
  };
}

/** The question a chat is waiting on, docked on top of the composer that answers it. */
export function QuestionDock({
  part,
  index,
  total,
  picked,
  disabled,
  hotkeys,
  searchTarget,
  onPick,
}: {
  part: ChatPart;
  index: number;
  total: number;
  picked: string[];
  disabled: boolean;
  /** Whether 1–9 pick an option while focus is outside a text field. */
  hotkeys: boolean;
  searchTarget: boolean;
  onPick: (label: string) => void;
}) {
  const { id: elementId } = useContext(WorkspaceDomContext);
  const root = useRef<HTMLDivElement>(null);
  const questionBox = useRef<HTMLDivElement>(null);
  /** The question's height once the user drags it; the next question starts at its default again. */
  const [height, setHeight] = useState<number | null>(null);
  /** Up to the chat header. */
  const [heightLimit, setHeightLimit] = useState(Number.POSITIVE_INFINITY);
  /** Until resized, the question takes what is left of 80% of the window once the options and the answer field have theirs. */
  const [questionMax, setQuestionMax] = useState(Number.POSITIVE_INFINITY);
  const prompt = part.prompt;
  const options = prompt?.options ?? [];
  const questionId = elementId(`question-${part.id}`);

  useLayoutEffect(() => {
    const dock = root.current;
    const question = questionBox.current;
    const pane = dock?.closest(".chat-pane");
    const composer = dock?.closest<HTMLElement>(".composer");
    if (!dock || !question || !pane || !composer) return;
    const measure = () => {
      setQuestionMax(Math.max(MIN_HEIGHT, Math.round(window.innerHeight * 0.8 - (composer.offsetHeight - question.offsetHeight))));
      const top = pane.querySelector(".chat-header")?.getBoundingClientRect().bottom ?? pane.getBoundingClientRect().top;
      setHeightLimit(question.offsetHeight + dock.getBoundingClientRect().top - top);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(pane);
    observer.observe(composer);
    return () => observer.disconnect();
  }, []);

  const latest = useRef({ options, onPick });
  latest.current = { options, onPick };
  useEffect(() => {
    if (!hotkeys || disabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (!(target instanceof HTMLElement) || target.isContentEditable || target.matches("input, textarea, select")) return;
      if (target !== document.body && !root.current?.closest(".chat-pane")?.contains(target)) return;
      const option = /^[1-9]$/.test(event.key) ? latest.current.options[Number(event.key) - 1] : undefined;
      if (!option) return;
      event.preventDefault();
      latest.current.onPick(option.label);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [hotkeys, disabled]);

  return (
    <div
      ref={root}
      className="chat-prompt-questions relative flex min-h-0 flex-col border-b border-b-border-variant"
    >
      <ResizeHandle
        label={m.chat_resize_question()}
        height={height}
        min={MIN_HEIGHT}
        max={heightLimit}
        measure={() => questionBox.current?.offsetHeight ?? MIN_HEIGHT}
        onChange={setHeight}
      />
      <div
        className="flex min-h-0 flex-1 flex-col outline-none"
        role="group"
        aria-labelledby={questionId}
        data-part-id={part.id}
        data-search-target={searchTarget || undefined}
        tabIndex={-1}
      >
        <div
          ref={questionBox}
          tabIndex={0}
          className="flex-[1_1000_auto] overflow-y-auto px-1.5 pt-3 pb-2 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-solid focus-visible:outline-focus"
          style={{ minHeight: MIN_HEIGHT, height: height ?? undefined, maxHeight: height === null ? questionMax : undefined }}
        >
          <div className="flex items-center gap-2 px-2.5 text-sm text-subtext">
            <span className="min-w-0 flex-1 truncate">{prompt?.header || m.chat_question()}</span>
            {total > 1 && (
              <span className="shrink-0 text-xs tabular-nums" aria-label={m.chat_question_progress({ current: index + 1, total })}>
                {index + 1}/{total}
              </span>
            )}
          </div>
          <div id={questionId} className="mt-1 px-2.5 text-base leading-snug text-text whitespace-pre-line wrap-anywhere">
            {prompt?.question?.replace(/\n\s*\n/g, "\n")}
          </div>
          {prompt?.multiSelect && <div className="px-2.5 pt-0.5 text-sm text-subtext">{m.chat_question_choose_any()}</div>}
        </div>
        {options.length > 0 && (
          <div className="flex min-h-0 shrink flex-col gap-1 overflow-y-auto px-1.5 pb-1.5">
            {options.map((option, optionIndex) => {
              const selected = picked.includes(option.label);
              return (
                <button
                  type="button"
                  key={option.label}
                  aria-pressed={selected}
                  disabled={disabled}
                  className={`flex w-full items-start gap-3 rounded-md px-2.5 py-2 text-start text-text cursor-pointer transition-colors duration-120 ease-standard focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-solid focus-visible:outline-focus disabled:cursor-default ${selected ? "bg-highlight" : "bg-surface enabled:hover:bg-panel"}`}
                  onClick={() => onPick(option.label)}
                >
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="text-base font-medium">{option.label}</span>
                    {option.description && option.description !== option.label && (
                      <span className="text-sm text-subtext">{option.description}</span>
                    )}
                  </span>
                  {(selected || optionIndex < 9) && (
                    <span
                      className={`mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-xs border text-xs tabular-nums ${selected ? "border-transparent" : "border-border text-subtext"}`}
                      aria-hidden="true"
                    >
                      {selected ? <Check size={16} /> : optionIndex + 1}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
