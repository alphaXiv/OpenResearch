import { m } from "../paraglide/messages.js";
import {
  Fragment,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { getSkillContent, type SkillInfo } from "../api";
import { splitCommandTokens } from "../planCommand";
import { Md } from "./Md";
import { Badge } from "./ui";

/** Metrics the composer mirror copies off the textarea so its text lands on the
 * real text glyph for glyph. */
const MIRRORED_PROPERTIES = [
  "font-family",
  "font-size",
  "font-weight",
  "font-style",
  "font-variant",
  "line-height",
  "letter-spacing",
  "word-spacing",
  "text-transform",
  "direction",
  "unicode-bidi",
  "tab-size",
  "padding-top",
  "padding-right",
  "padding-bottom",
  "padding-left",
  "border-top-width",
  "border-right-width",
  "border-bottom-width",
  "border-left-width",
];

const skillContentCache = new Map<string, Promise<string>>();

function loadSkillContent(name: string, projectId: string): Promise<string> {
  const key = `${projectId}\u0000${name}`;
  const cached = skillContentCache.get(key);
  if (cached) return cached;
  const request = getSkillContent(name, projectId).catch((error: unknown) => {
    skillContentCache.delete(key);
    throw error;
  });
  skillContentCache.set(key, request);
  return request;
}

function chipSegments(
  text: string,
  isCommand: (name: string) => boolean,
  chipClassName: string,
  onCommandMouseDown?: (
    event: ReactMouseEvent<HTMLSpanElement>,
    end: number,
  ) => void,
  renderCommand?: (
    label: string,
    name: string,
    end: number,
    key: number,
  ) => ReactNode,
  wrapPlainText = false,
): ReactNode[] {
  let offset = 0;
  return splitCommandTokens(text, isCommand).map((segment, i) => {
    const end = offset + segment.text.length;
    offset = end;
    const name = segment.text.slice(1).toLowerCase();
    if (segment.command && renderCommand) {
      return renderCommand(segment.text, name, end, i);
    }
    return segment.command ? (
      <span
        key={i}
        className={chipClassName}
        onMouseDown={
          onCommandMouseDown ? (event) => onCommandMouseDown(event, end) : undefined
        }
      >
        <span className="text-skill-blue-slash">/</span>
        {segment.text.slice(1)}
      </span>
    ) : (
      wrapPlainText ? (
        <span key={i} aria-hidden="true">{segment.text}</span>
      ) : (
        <Fragment key={i}>{segment.text}</Fragment>
      )
    );
  });
}

function ComposerSkillToken({
  label,
  name,
  end,
  skill,
  projectId,
  textareaRef,
}: {
  label: string;
  name: string;
  end: number;
  skill: SkillInfo;
  projectId: string;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
}) {
  const tokenRef = useRef<HTMLSpanElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<number | null>(null);
  const cardId = useId();
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({});

  const clearClose = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const placeCard = () => {
    const token = tokenRef.current;
    if (!token) return;
    const rect = token.getBoundingClientRect();
    const width = Math.min(420, window.innerWidth - 32);
    const left = Math.max(16, Math.min(rect.left - 4, window.innerWidth - width - 16));
    setPosition(
      rect.top > 300
        ? { bottom: window.innerHeight - rect.top + 12, left, width }
        : { left, top: rect.bottom + 12, width },
    );
  };
  const show = () => {
    clearClose();
    placeCard();
    setOpen(true);
    if (content !== null || loading) return;
    setLoading(true);
    loadSkillContent(name, projectId)
      .then(setContent)
      .catch(() => setContent(null))
      .finally(() => setLoading(false));
  };
  const scheduleClose = () => {
    clearClose();
    closeTimer.current = window.setTimeout(() => setOpen(false), 120);
  };

  useEffect(() => () => clearClose(), []);
  useEffect(() => {
    if (!open) return;
    const update = () => placeCard();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [open]);

  return (
    <Fragment>
      <span
        ref={tokenRef}
        role="button"
        tabIndex={0}
        aria-controls={cardId}
        aria-expanded={open}
        aria-label={m.a11y_preview_skill({ name })}
        className="composer-chip group/skill pointer-events-auto relative z-1 cursor-text rounded-md bg-background text-skill-blue"
        onMouseEnter={show}
        onMouseLeave={scheduleClose}
        onFocus={show}
        onBlur={scheduleClose}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            setOpen(false);
            return;
          }
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            show();
            return;
          }
          if (open && (event.key === "ArrowDown" || event.key === "PageDown")) {
            event.preventDefault();
            cardRef.current?.scrollBy({
              top: event.key === "PageDown" ? 240 : 48,
              behavior: "smooth",
            });
          }
          if (open && (event.key === "ArrowUp" || event.key === "PageUp")) {
            event.preventDefault();
            cardRef.current?.scrollBy({
              top: event.key === "PageUp" ? -240 : -48,
              behavior: "smooth",
            });
          }
        }}
        onMouseDown={(event) => {
          event.preventDefault();
          textareaRef.current?.focus();
          textareaRef.current?.setSelectionRange(end, end);
          clearClose();
        }}
      >
        <span className="pointer-events-none absolute -inset-[7px] z-0 rounded-md bg-skill-blue-subtle opacity-0 transition-opacity group-hover/skill:opacity-100" />
        <span className="relative z-1">
          <span className="text-skill-blue-slash">/</span>
          {label.slice(1)}
        </span>
      </span>
      {open &&
        createPortal(
          <div
            id={cardId}
            ref={cardRef}
            role="dialog"
            aria-label={m.a11y_skill({ name })}
            style={{
              ...position,
              maxHeight: "min(28rem, calc(100vh - 2rem))",
            }}
            className="fixed z-100 overflow-y-auto rounded-lg border border-border bg-background shadow-floating"
            onMouseEnter={clearClose}
            onMouseLeave={scheduleClose}
            onFocus={clearClose}
            onBlur={scheduleClose}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <div className="sticky top-0 z-1 flex items-center gap-2 border-b border-border-variant bg-background px-4 py-3">
              <span className="text-sm font-medium text-muted">/{name}</span>
              <Badge className="h-5 border-border-variant bg-canvas px-1.5 tracking-[0.05em]">
                {m.skill_chips_badge()}
              </Badge>
            </div>
            <div className="p-4 text-sm text-text">
              {loading && content === null ? (
                <span className="text-muted">{m.skill_chips_loading_skill()}</span>
              ) : (
                <Md text={content ?? skill.description} />
              )}
            </div>
          </div>,
          document.body,
        )}
    </Fragment>
  );
}

/** A sent message's text with every known `/command` rendered as a chip. */
export function MessageWithChips({
  text,
  isCommand,
}: {
  text: string;
  isCommand: (name: string) => boolean;
}) {
  return (
    <>
      {chipSegments(
        text,
        isCommand,
        "skill-chip mx-1 inline-flex items-center rounded-md px-2 py-1 font-medium text-skill-blue transition-colors hover:bg-skill-blue-subtle",
      )}
    </>
  );
}

/** Chips for the composer, aligned to the textarea's text by a mirror
 * that reproduces its wrapping exactly — a textarea cannot style one range of
 * its value. Requires the positioned parent's only in-flow child to be a
 * textarea that renders BEFORE this (its ref must be attached when the mirror
 * measures it). Plain mirror runs stay transparent; only tokens paint above the
 * native input. */
export function ComposerSkillChips({
  text,
  isCommand,
  skills,
  projectId,
  textareaRef,
}: {
  /** The textarea's exact current value — chips land by character offset. */
  text: string;
  isCommand: (name: string) => boolean;
  skills: SkillInfo[];
  projectId: string;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
}) {
  const mirrorRef = useRef<HTMLDivElement>(null);

  // Out of flow, so writing the mirror's styles here cannot resize the textarea
  // the observer watches.
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    const mirror = mirrorRef.current;
    if (!textarea || !mirror) return;
    const sync = () => {
      const computed = getComputedStyle(textarea);
      for (const property of MIRRORED_PROPERTIES)
        mirror.style.setProperty(property, computed.getPropertyValue(property));
      // clientWidth excludes the scrollbar, so the mirror wraps where the textarea does.
      mirror.style.width = `${
        textarea.clientWidth +
        parseFloat(computed.borderLeftWidth) +
        parseFloat(computed.borderRightWidth)
      }px`;
    };
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(textarea);
    return () => observer.disconnect();
  }, [text, textareaRef]);

  // The chips ride the textarea's own scrolling — caret-driven (no scroll event
  // on the frame the text changes) as well as user-driven.
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const sync = () => {
      if (mirrorRef.current) mirrorRef.current.scrollTop = textarea.scrollTop;
    };
    sync();
    textarea.addEventListener("scroll", sync);
    return () => textarea.removeEventListener("scroll", sync);
  }, [textareaRef, text]);

  return (
    <div
      ref={mirrorRef}
      className="composer-chips pointer-events-none absolute inset-y-0 start-0 z-2 box-border overflow-hidden whitespace-pre-wrap break-words border-solid border-transparent text-transparent select-none"
    >
      {/* Hover padding is painted outside the mirrored text box so it cannot
        * shift the textarea's following glyphs. */}
      {chipSegments(
        text,
        isCommand,
        "",
        undefined,
        (label, name, end, key) => {
          const skill = skills.find((candidate) => candidate.name === name);
          return skill && skill.source !== "command" ? (
            <ComposerSkillToken
              key={`${key}:${end}`}
              label={label}
              name={name}
              end={end}
              skill={skill}
              projectId={projectId}
              textareaRef={textareaRef}
           />
          ) : (
            <span key={`${key}:${end}`} aria-hidden="true" className="bg-background text-skill-blue">
              <span className="text-skill-blue-slash">/</span>
              {label.slice(1)}
            </span>
          );
        },
        true,
      )}
      {/* A trailing newline drops its line box here but not in the textarea,
        * which would clamp the mirror's scrollTop a line short. */}
      {"\u200b"}
    </div>
  );
}
