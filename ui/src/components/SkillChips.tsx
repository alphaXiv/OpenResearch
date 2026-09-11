import { useQuery } from "@tanstack/react-query";
import { ListChecks, WandSparkles } from "lucide-react";

import { getSkillContentQuery } from "../queries/settings";
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
import { type SkillInfo } from "../api";
import { commandDisplayName, splitCommandTokens } from "../planCommand";
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

let skillMeasurement: CanvasRenderingContext2D | null = null;

export function skillMarginSpaces(name: string, textarea: HTMLTextAreaElement | null): number {
  const measurement = skillMeasurement ??= document.createElement("canvas").getContext("2d");
  if (!measurement || !textarea) return 6;
  const style = getComputedStyle(textarea);
  measurement.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  // Match SkillLabel's 16px icon and 4px gap, plus 6px before adjacent text.
  const extraWidth = 16 + 4 + measurement.measureText(commandDisplayName(name)).width
    - measurement.measureText(`/${name}`).width;
  return Math.max(2, Math.ceil((extraWidth + 6) / measurement.measureText(" ").width));
}

function SkillLabel({ name }: { name: string }) {
  const Icon = name === "plan" ? ListChecks : WandSparkles;
  return (
    <>
      <Icon size={16} strokeWidth={1.5} className="me-1 inline-block align-middle" aria-hidden="true" />
      {commandDisplayName(name)}
    </>
  );
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
  return splitCommandTokens(text, isCommand).map((segment, i, segments) => {
    const end = offset + segment.text.length;
    offset = end;
    const name = segment.text.slice(1).toLowerCase();
    if (segment.command && renderCommand) {
      return renderCommand(segment.text, name, end, i);
    }
    let plainText = segment.text;
    if (!wrapPlainText) {
      if (segments[i - 1]?.command) plainText = plainText.replace(/^[ \t]+/, " ");
      if (segments[i + 1]?.command) plainText = plainText.replace(/[ \t]+$/, " ");
    }
    return segment.command ? (
      <span
        key={i}
        className={chipClassName}
        onMouseDown={
          onCommandMouseDown ? (event) => onCommandMouseDown(event, end) : undefined
        }
      >
        <SkillLabel name={name} />
      </span>
    ) : (
      wrapPlainText ? (
        <span key={i} aria-hidden="true">{segment.text}</span>
      ) : (
        <Fragment key={i}>{plainText}</Fragment>
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
  const preview = useQuery({ ...getSkillContentQuery(name, projectId, skill.harness), enabled: open, subscribed: open });
  const content = preview.data ?? null;
  const loading = preview.isFetching;
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
        className="composer-chip group/skill pointer-events-auto relative z-1 inline-grid align-baseline cursor-text rounded-md bg-background text-skill-blue"
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
        {/* Keep the native token's width; the label uses the spacing reserved on selection. */}
        <span className="invisible col-start-1 row-start-1" aria-hidden="true">{label}</span>
        <span className="relative z-1 col-start-1 row-start-1 w-0 whitespace-nowrap">
          <span className="bg-background text-skill-blue">
            <SkillLabel name={name} />
          </span>
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
        "skill-chip me-0.5 whitespace-nowrap font-normal text-skill-blue",
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
  editingTokenEnd,
  isCommand,
  skills,
  projectId,
  textareaRef,
}: {
  /** The textarea's exact current value — chips land by character offset. */
  text: string;
  editingTokenEnd?: number;
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
          const trailing = text.slice(end);
          const spaceCount = /^[ \t]+/.exec(trailing)?.[0].length ?? 0;
          if (end === editingTokenEnd || (trailing && !trailing.startsWith("\n") && spaceCount < skillMarginSpaces(name, textareaRef.current))) {
            return <span key={`${key}:${end}`} aria-hidden="true">{label}</span>;
          }
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
