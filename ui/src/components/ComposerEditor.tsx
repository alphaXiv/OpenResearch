import { defaultKeymap, history, historyKeymap, insertNewlineAndIndent } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { languages } from "@codemirror/language-data";
import {
  Compartment,
  EditorState,
  Prec,
  StateEffect,
  StateField,
  type Extension,
  type Range,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  keymap,
  placeholder as placeholderText,
  type DecorationSet,
} from "@codemirror/view";
import { FileText, Folder, MessageSquare } from "lucide-react";
import {
  useContext,
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type Ref,
} from "react";
import { createPortal } from "react-dom";
import { createRoot, type Root } from "react-dom/client";

import { fmtBytes, type CitedMessage, type HarnessId, type SkillInfo } from "../api";
import { CITED_ID, refLabel } from "../agentMessages";
import { commandDisplayName } from "../composerCommands";
import type { ComposerAttachment } from "../composerStash";
import { PAPER_LINK, splitMentionPath } from "../fileMentions";
import { livePreview, pointerSelection, revealChanged, revealedSpan } from "../markdownLivePreview";
import { m } from "../paraglide/messages.js";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { FileTypeIcon } from "./FileTypeIcon";
import { HarnessLogo } from "./HarnessLogo";
import { LitSourceLogo } from "./LitSourceLogo";
import { CommandIcon, SkillPreviewCard } from "./SkillChips";

/** What the draft's tokens resolve to; a token that resolves to nothing stays text. */
export interface ChipSource {
  isCommand: (name: string) => boolean;
  /** The skill a command chip previews, if any. */
  skill: (name: string) => SkillInfo | undefined;
  /** Who wrote the message a `msg_` ref cites, and its first words. */
  message: (ref: string) => CitedMessage | undefined;
  /** A chat in this project, by id. */
  chat: (id: string) => { title: string; harness: HarnessId } | undefined;
  isPath: (path: string) => boolean;
  attachment: (id: number) => ComposerAttachment | undefined;
}

type Chip =
  | { kind: "command"; name: string; previewable: boolean }
  | { kind: "file"; path: string }
  | { kind: "message"; ref: string; cited: CitedMessage }
  | { kind: "chat"; id: string; title: string; harness: HarnessId }
  | { kind: "paper"; title: string; url: string }
  | { kind: "attachment"; attachment: ComposerAttachment };

const CHIP_TOKEN = new RegExp(
  [
    String.raw`\[(?:Image|PDF) #(?<attachment>\d+)\]`,
    PAPER_LINK.source,
    `(?<cited>${CITED_ID.source})`,
    String.raw`(?<=^|\s)(?:\/(?<command>[^\s/]+)|@"(?<quotedPath>[^"\n]+)"|@(?<path>\S+))(?=\s|$)`,
  ].join("|"),
  "g",
);

function findChips(text: string, source: ChipSource): { from: number; to: number; chip: Chip }[] {
  const chips: { from: number; to: number; chip: Chip }[] = [];
  for (const match of text.matchAll(CHIP_TOKEN)) {
    const { attachment: attachmentId, paperTitle, paperUrl, cited, command, quotedPath, path } = match.groups ?? {};
    let chip: Chip | null = null;
    if (attachmentId) {
      const attachment = source.attachment(Number(attachmentId));
      if (attachment) chip = { kind: "attachment", attachment };
    } else if (paperTitle && paperUrl) {
      chip = { kind: "paper", title: paperTitle, url: paperUrl };
    } else if (cited?.startsWith("msg_")) {
      const message = source.message(cited);
      if (message) chip = { kind: "message", ref: cited, cited: message };
    } else if (cited) {
      const chat = source.chat(cited);
      if (chat) chip = { kind: "chat", id: cited, ...chat };
    } else if (command) {
      const name = command.toLowerCase();
      if (source.isCommand(name)) chip = { kind: "command", name, previewable: source.skill(name) !== undefined };
    } else if (source.isPath(quotedPath ?? path)) {
      chip = { kind: "file", path: quotedPath ?? path };
    }
    if (chip) chips.push({ from: match.index, to: match.index + match[0].length, chip });
  }
  return chips;
}

function chipKey(chip: Chip): string {
  if (chip.kind === "command") return `/${chip.name}`;
  if (chip.kind === "file") return `@${chip.path}`;
  if (chip.kind === "message") return chip.ref;
  if (chip.kind === "chat") return chip.id;
  return chip.kind === "paper" ? chip.url : `#${chip.attachment.id}`;
}

/** 20px tall and 3px down in the editor's 24px line, so the label sits on the
 * prose baseline without growing the line. */
const CHIP_CLASS =
  "mx-px mt-[3px] inline-flex h-5 max-w-64 cursor-default items-center gap-1 rounded-md border px-1.5 align-top text-sm leading-4.5 whitespace-nowrap";

function ChipContent({ chip }: { chip: Chip }) {
  if (chip.kind === "command") {
    return (
      <>
        <CommandIcon name={chip.name} className="size-3.5 shrink-0" />
        <span className="min-w-0 truncate">{commandDisplayName(chip.name)}</span>
      </>
    );
  }
  if (chip.kind === "file") {
    const { name } = splitMentionPath(chip.path);
    return (
      <>
        {chip.path.endsWith("/") ? <Folder size={14} className="shrink-0 text-subtext" aria-hidden="true" /> : <FileTypeIcon name={name} />}
        <span className="min-w-0 truncate">{name}</span>
      </>
    );
  }
  if (chip.kind === "chat") {
    return (
      <>
        <HarnessLogo harness={chip.harness} size={14} />
        <span className="min-w-0 truncate">{chip.title}</span>
      </>
    );
  }
  if (chip.kind === "paper") {
    return (
      <>
        <LitSourceLogo source="alphaxiv" size={14} decorative />
        <span className="min-w-0 truncate">{chip.title}</span>
      </>
    );
  }
  if (chip.kind === "message") {
    return (
      <>
        {chip.cited.harness
          ? <HarnessLogo harness={chip.cited.harness} size={14} />
          : <MessageSquare size={14} className="shrink-0 text-subtext" aria-hidden="true" />}
        <span className="min-w-0 truncate">{chip.cited.preview || refLabel(chip.ref)}</span>
      </>
    );
  }
  const { attachment } = chip;
  return (
    <>
      {attachment.mediaType.startsWith("image/") ? (
        <img src={attachment.dataUrl} alt="" className="size-4 shrink-0 rounded-xs object-cover" />
      ) : (
        <FileText size={14} className="shrink-0 text-subtext" aria-hidden="true" />
      )}
      <span className="min-w-0 truncate">{attachment.name ?? m.chat_pasted_image()}</span>
      <span className="shrink-0 text-muted">{fmtBytes(attachment.size)}</span>
    </>
  );
}

const chipRoots = new WeakMap<HTMLElement, Root>();

class ChipWidget extends WidgetType {
  readonly chip: Chip;

  constructor(chip: Chip) {
    super();
    this.chip = chip;
  }

  override eq(other: ChipWidget) {
    return chipKey(other.chip) === chipKey(this.chip) && this.previewable === other.previewable;
  }

  get previewable() {
    return this.chip.kind === "command" && this.chip.previewable;
  }

  override toDOM() {
    const span = document.createElement("span");
    span.className = `${CHIP_CLASS} ${
      this.chip.kind === "command"
        ? "border-transparent bg-skill-blue-subtle text-skill-blue"
        : "border-border bg-surface text-text"
    }`;
    span.dataset.chip = chipKey(this.chip);
    if (this.chip.kind === "file") span.title = this.chip.path;
    if (this.chip.kind === "message") span.title = this.chip.cited.preview;
    if (this.chip.kind === "paper") span.title = this.chip.url;
    if (this.chip.kind === "command" && this.chip.previewable) {
      // A tab stop, so keyboard users can open the preview that hover shows.
      span.tabIndex = 0;
      span.role = "button";
      span.ariaLabel = m.a11y_preview_skill({ name: this.chip.name });
    }
    const root = createRoot(span);
    root.render(<ChipContent chip={this.chip} />);
    chipRoots.set(span, root);
    return span;
  }

  override destroy(dom: HTMLElement) {
    const root = chipRoots.get(dom);
    // CodeMirror can destroy widgets inside a React commit, where a synchronous unmount warns.
    queueMicrotask(() => root?.unmount());
  }

  /** Pointer events place the caret; keys typed on a focused chip stay with it. */
  override ignoreEvent(event: Event) {
    return event.type.startsWith("key");
  }
}

const refreshChips = StateEffect.define<null>();

function chipDecorations(state: EditorState, source: ChipSource): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const { from, to, chip } of findChips(state.doc.toString(), source)) {
    // Typed tokens reveal under the caret so they stay editable; inserted ones never do.
    if ((chip.kind === "command" || chip.kind === "file") && revealedSpan(state, from, to)) continue;
    ranges.push(Decoration.replace({ widget: new ChipWidget(chip) }).range(from, to));
  }
  return Decoration.set(ranges);
}

/** Chips over `/command`, `@path` and attachment tokens. Shown chips are atomic,
 * so the caret steps over them and Backspace takes one whole — also from just
 * past the space that follows one. */
function chips(source: () => ChipSource): Extension {
  const field = StateField.define<DecorationSet>({
    create: (state) => chipDecorations(state, source()),
    update: (value, transaction) =>
      revealChanged(transaction) || transaction.effects.some((effect) => effect.is(refreshChips))
        ? chipDecorations(transaction.state, source())
        : value,
    provide: (self) => [
      EditorView.decorations.from(self),
      EditorView.atomicRanges.of((view) => view.state.field(self)),
    ],
  });
  const deleteChipBehindSpace = (view: EditorView) => {
    const { state } = view;
    const { head, empty } = state.selection.main;
    if (!empty || !/^[ \t]$/.test(state.sliceDoc(head - 1, head))) return false;
    let chipFrom = -1;
    state.field(field).between(head - 1, head - 1, (from, to) => {
      if (to === head - 1) chipFrom = from;
    });
    if (chipFrom < 0) return false;
    view.dispatch({ changes: { from: chipFrom, to: head }, scrollIntoView: true, userEvent: "delete.backward" });
    return true;
  };
  return [pointerSelection, Prec.high(field), Prec.high(keymap.of([{ key: "Backspace", run: deleteChipBehindSpace }]))];
}

/** Natural height stops at eight lines, then the editor scrolls. */
const composerTheme = EditorView.theme({
  "&": { color: "var(--text)", maxHeight: "calc(12em + 2 * var(--editor-pad-y))" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "inherit", lineHeight: "1.5", overflow: "auto" },
  // WebKit can retain an empty horizontal scrollbar gutter after the draft shrinks.
  ".cm-scroller::-webkit-scrollbar": { height: "0" },
  // `minWidth` lets WebKit shrink the content below the placeholder's one-line width.
  ".cm-content": { padding: "var(--editor-pad-y) 0", caretColor: "var(--text)", minWidth: "0" },
  ".cm-line": { padding: "0 var(--editor-pad-x)" },
  // One line however narrow the box, so an empty draft keeps the one-line height.
  ".cm-placeholder": { color: "var(--muted)", maxWidth: "100%", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  "&.cm-plain .cm-content": { fontFamily: "var(--mono)", fontSize: "0.875em" },
});

// Its keymap would continue lists on Enter, which sends; `livePreview` binds Shift+Enter instead.
const markdownSource = markdown({
  addKeymap: false,
  base: markdownLanguage,
  codeLanguages: languages,
  completeHTMLTags: false,
  extensions: { remove: ["HTMLBlock", "HTMLTag"] },
});

function modeExtension(plain: boolean): Extension {
  return plain ? EditorView.editorAttributes.of({ class: "cm-plain" }) : [markdownSource, livePreview];
}

function isComposing(event: KeyboardEvent): boolean {
  // WebKit can end composition before the confirming keydown (keyCode 229).
  return event.isComposing || event.keyCode === 229;
}

/** One 24px line plus the 10px padding above and below; a shorter box would scroll an empty draft. */
const MIN_HEIGHT = 44;
const KEYBOARD_STEP = 24;

function storedHeight(key: string): number | null {
  try {
    const height = Number(localStorage.getItem(key) ?? NaN);
    return Number.isFinite(height) && height >= MIN_HEIGHT ? height : null;
  } catch {
    return null;
  }
}

/** Dragged heights stop short of crowding out the transcript. */
function maxHeight(dom: HTMLElement): number {
  const pane = dom.closest<HTMLElement>(".chat-pane");
  return Math.max(MIN_HEIGHT, Math.min(window.innerHeight * 0.6, (pane?.clientHeight ?? window.innerHeight) - 220));
}

export interface ComposerEditorHandle {
  readonly dom: HTMLElement | null;
  focus: (at?: "end") => void;
  /** An undoable edit replacing the draft, with the caret at `cursor`. */
  setText: (text: string, cursor: number) => void;
  /** Insert a chip token at the caret, spaced from its neighbors. */
  insertToken: (token: string) => void;
}

/**
 * The composer's CodeMirror markdown editor. `value` is controlled: a value the
 * editor did not produce replaces the document and its undo history, as when
 * the composer switches chats.
 */
export function ComposerEditor({
  ref,
  value,
  onChange,
  onKeyDown,
  onSubmit,
  onFiles,
  onPaste,
  placeholder,
  plain,
  chipSource,
  projectId,
  describedBy,
  draftKey,
  resizeKey,
}: {
  ref: Ref<ComposerEditorHandle>;
  value: string;
  /** Every document or caret change; `typed` marks text the user typed or pasted. */
  onChange: (value: string, cursor: number, typed: boolean) => void;
  /** Runs before the editor's own keys; call `preventDefault` to claim the key. */
  onKeyDown: (event: KeyboardEvent) => void;
  /** Enter without Shift, outside a code block. */
  onSubmit: (event: KeyboardEvent) => void;
  /** Dropped files. */
  onFiles: (files: File[]) => void;
  /** Runs before the editor's own paste; call `preventDefault` to claim it. */
  onPaste: (event: ClipboardEvent) => void;
  placeholder: string;
  /** Shell commands: monospace, no markdown or chips. */
  plain: boolean;
  chipSource: ChipSource;
  projectId: string;
  describedBy?: string;
  /** Which draft `value` belongs to; a new key starts a fresh undo history even when the text matches. */
  draftKey: string;
  /** Where the height the user drags to is remembered. */
  resizeKey: string;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const latest = useRef({ onChange, onKeyDown, onSubmit, onFiles, onPaste, chipSource, plain, placeholder, describedBy });
  latest.current = { onChange, onKeyDown, onSubmit, onFiles, onPaste, chipSource, plain, placeholder, describedBy };
  const [compartments] = useState(() => ({ mode: new Compartment(), placeholder: new Compartment(), aria: new Compartment() }));
  const [hovered, setHovered] = useState<HTMLElement | null>(null);
  const previewId = useId();
  /** A height the user dragged to; null keeps the natural height. */
  const [height, setHeight] = useState(() => storedHeight(resizeKey));
  const [heightLimit, setHeightLimit] = useState(Number.POSITIVE_INFINITY);
  const closeTimer = useRef<number | null>(null);

  const ariaAttributes = () => ({
    "aria-label": latest.current.placeholder,
    ...(latest.current.describedBy ? { "aria-describedby": latest.current.describedBy } : {}),
    spellcheck: "true",
    writingsuggestions: "false",
  });

  const createState = (doc: string) =>
    EditorState.create({
      doc,
      selection: { anchor: doc.length },
      extensions: [
        history(),
        Prec.highest(EditorView.domEventHandlers({
          keydown(event) {
            if (isComposing(event)) return false;
            latest.current.onKeyDown(event);
            return event.defaultPrevented;
          },
          paste(event) {
            latest.current.onPaste(event);
            return event.defaultPrevented;
          },
          dragover(event) {
            if (!event.dataTransfer?.types.includes("Files")) return false;
            event.preventDefault();
            return true;
          },
          drop(event, view) {
            const files = Array.from(event.dataTransfer?.files ?? []);
            if (files.length === 0) return false;
            event.preventDefault();
            const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (pos !== null) view.dispatch({ selection: { anchor: pos } });
            latest.current.onFiles(files);
            return true;
          },
        })),
        compartments.mode.of(modeExtension(latest.current.plain)),
        keymap.of([
          // Enter belongs to send; Shift+Enter breaks the line.
          ...defaultKeymap.filter((binding) => !binding.key?.endsWith("Enter")),
          ...historyKeymap,
          { key: "Shift-Enter", run: insertNewlineAndIndent },
        ]),
        Prec.lowest(EditorView.domEventHandlers({
          keydown(event) {
            if (event.key !== "Enter" || event.shiftKey || isComposing(event)) return false;
            event.preventDefault();
            latest.current.onSubmit(event);
            return true;
          },
        })),
        chips(() => latest.current.chipSource),
        EditorView.lineWrapping,
        EditorView.perLineTextDirection.of(true),
        compartments.placeholder.of(placeholderText(latest.current.placeholder)),
        compartments.aria.of(EditorView.contentAttributes.of(ariaAttributes())),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged && !update.selectionSet) return;
          const typed = update.transactions.some(
            (transaction) => transaction.isUserEvent("input") && !transaction.isUserEvent("input.type.compose"),
          );
          latest.current.onChange(update.state.doc.toString(), update.state.selection.main.head, typed);
        }),
        composerTheme,
      ],
    });

  useLayoutEffect(() => {
    const view = new EditorView({ state: createState(value), parent: hostRef.current! });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
    // Created once; later props flow through `latest` and the compartments.
  }, []);

  const shownDraftKey = useRef(draftKey);
  useLayoutEffect(() => {
    const view = viewRef.current;
    if (!view || (view.state.doc.toString() === value && shownDraftKey.current === draftKey)) return;
    shownDraftKey.current = draftKey;
    view.setState(createState(value));
    latest.current.onChange(value, value.length, false);
  }, [value, draftKey]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.mode.reconfigure(modeExtension(plain)) });
  }, [plain, compartments]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: [
        compartments.placeholder.reconfigure(placeholderText(placeholder)),
        compartments.aria.reconfigure(EditorView.contentAttributes.of(ariaAttributes())),
      ],
    });
  }, [placeholder, describedBy, compartments]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: refreshChips.of(null) });
  }, [chipSource]);

  useLayoutEffect(() => {
    const dom = viewRef.current?.dom;
    if (!dom) return;
    const clamped = height === null ? null : Math.min(height, heightLimit);
    dom.style.height = clamped === null ? "" : `${clamped}px`;
    dom.style.maxHeight = clamped === null ? "" : "none";
  }, [height, heightLimit]);

  useEffect(() => {
    try {
      if (height === null) localStorage.removeItem(resizeKey);
      else localStorage.setItem(resizeKey, String(height));
    } catch {
      // Storage can be unavailable; the height then lasts until reload.
    }
  }, [resizeKey, height]);

  useEffect(() => {
    const dom = viewRef.current?.dom;
    if (!dom) return;
    const measure = () => setHeightLimit(maxHeight(dom));
    measure();
    const pane = dom.closest(".chat-pane");
    const observer = new ResizeObserver(measure);
    if (pane) observer.observe(pane);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);

  const resizeBy = (start: number, delta: number) =>
    setHeight(Math.round(Math.min(Math.max(start + delta, MIN_HEIGHT), heightLimit)));
  const currentHeight = () => viewRef.current?.dom.getBoundingClientRect().height ?? MIN_HEIGHT;

  useImperativeHandle(ref, () => ({
    get dom() {
      return viewRef.current?.dom ?? null;
    },
    focus(at) {
      const view = viewRef.current;
      if (!view) return;
      view.focus();
      if (at === "end") view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
    },
    setText(text, cursor) {
      const view = viewRef.current;
      if (!view) return;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: text },
        selection: { anchor: cursor },
        scrollIntoView: true,
      });
      view.focus();
    },
    insertToken(token) {
      const view = viewRef.current;
      if (!view) return;
      // At the caret, never over a selection: files land after an async read, when the selection may be new.
      const at = view.state.selection.main.head;
      const before = view.state.sliceDoc(at - 1, at);
      const after = view.state.sliceDoc(at, at + 1);
      const insert = `${before && !/\s/.test(before) ? " " : ""}${token}${/\s/.test(after) ? "" : " "}`;
      view.dispatch({ changes: { from: at, insert }, selection: { anchor: at + insert.length }, scrollIntoView: true });
    },
  }), []);

  const keepOpen = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const scheduleClose = () => {
    keepOpen();
    closeTimer.current = window.setTimeout(() => setHovered(null), 120);
  };
  useEffect(() => keepOpen, []);

  const hoveredKey = hovered?.isConnected ? hovered.dataset.chip ?? "" : "";
  const hoveredSkill = hoveredKey.startsWith("/") ? chipSource.skill(hoveredKey.slice(1)) : undefined;
  const hoveredAttachment = hoveredKey.startsWith("#") ? chipSource.attachment(Number(hoveredKey.slice(1))) : undefined;

  return (
    <div
      ref={hostRef}
      className="composer-editor min-w-0 flex-1 [--editor-pad-x:12px] [--editor-pad-y:10px]"
      onKeyDown={(event) => {
        // CodeMirror skips its handlers while an IME composes; keep those keys
        // (an Escape that cancels a candidate) from reaching page shortcuts.
        if (isComposing(event.nativeEvent) || viewRef.current?.composing) {
          event.stopPropagation();
          return;
        }

        const chip = event.target instanceof HTMLElement && event.target.matches("[data-chip]") ? event.target : null;
        if (!chip) return;
        const scroll = { ArrowDown: 48, ArrowUp: -48, PageDown: 240, PageUp: -240 }[event.key];
        if (event.key === "Escape" && hovered === chip) {
          event.preventDefault();
          event.stopPropagation();
          setHovered(null);
        } else if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          keepOpen();
          setHovered(chip);
        } else if (scroll !== undefined) {
          event.preventDefault();
          document.getElementById(previewId)?.scrollBy({ top: scroll, behavior: "smooth" });
        }
      }}
      onFocus={(event) => {
        if (!(event.target instanceof HTMLElement) || !event.target.matches("[data-chip]")) return;
        keepOpen();
        setHovered(event.target);
      }}
      onBlur={(event) => {
        if (event.target instanceof Element && event.target.matches("[data-chip]")) scheduleClose();
      }}
      onMouseOver={(event) => {
        const chip = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-chip]") : null;
        if (!chip) return;
        keepOpen();
        setHovered(chip);
      }}
      onMouseOut={(event) => {
        if (event.target instanceof Element && event.target.closest("[data-chip]")) scheduleClose();
      }}
    >
      {/* Positioned against the composer box, so it rides the box's top edge. */}
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label={m.chat_resize_composer()}
        aria-valuemin={MIN_HEIGHT}
        aria-valuemax={Number.isFinite(heightLimit) ? Math.round(heightLimit) : undefined}
        aria-valuenow={Math.round(height ?? MIN_HEIGHT)}
        title={m.chat_resize_composer_hint()}
        tabIndex={0}
        className="group/resize absolute inset-x-0 -top-1.5 z-3 flex h-3 cursor-row-resize touch-none items-center justify-center outline-none"
        onPointerDown={(event) => {
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          const startY = event.clientY;
          const start = currentHeight();
          const move = (moveEvent: PointerEvent) => resizeBy(start, startY - moveEvent.clientY);
          const stop = () => {
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", stop);
            window.removeEventListener("pointercancel", stop);
          };
          window.addEventListener("pointermove", move);
          window.addEventListener("pointerup", stop);
          window.addEventListener("pointercancel", stop);
        }}
        onDoubleClick={() => setHeight(null)}
        onKeyDown={(event) => {
          if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            event.preventDefault();
            resizeBy(height ?? currentHeight(), event.key === "ArrowUp" ? KEYBOARD_STEP : -KEYBOARD_STEP);
          } else if (event.key === "Home") {
            event.preventDefault();
            setHeight(null);
          }
        }}
      >
        <span className="h-1 w-10 rounded-full bg-muted opacity-0 transition-opacity group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100 group-active/resize:opacity-100" />
      </div>
      {hovered && hoveredSkill && (
        <SkillPreviewCard
          id={previewId}
          anchor={hovered}
          name={hoveredKey.slice(1)}
          skill={hoveredSkill}
          projectId={projectId}
          onMouseEnter={keepOpen}
          onMouseLeave={scheduleClose}
        />
      )}
      {hovered && hoveredAttachment?.mediaType.startsWith("image/") && (
        <ImagePreview anchor={hovered} attachment={hoveredAttachment} />
      )}
    </div>
  );
}

function ImagePreview({ anchor, attachment }: { anchor: HTMLElement; attachment: ComposerAttachment }) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const [position, setPosition] = useState<CSSProperties>({});
  useLayoutEffect(() => {
    const rect = anchor.getBoundingClientRect();
    setPosition({ left: Math.max(16, rect.left), bottom: window.innerHeight - rect.top + 8 });
  }, [anchor]);
  return createPortal(
    <img
      src={attachment.dataUrl}
      alt={attachment.name ?? m.chat_pasted_image()}
      style={position}
      className="pointer-events-none fixed z-100 max-h-60 max-w-80 rounded-md border border-border bg-background object-contain shadow-dropdown"
    />,
    portalContainer ?? document.body,
  );
}
