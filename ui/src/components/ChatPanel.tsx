import { m } from "../paraglide/messages.js";
import { autoDir, ltr } from "../i18n";
import { useLocale } from "../locale";
import {
  ArrowDown,
  ArrowUpRight,
  Blocks,
  BookOpen,
  Check,
  ChevronLeft,
  ChevronRight,
  CircleX,
  Clock,
  CornerDownLeft,
  FileText,
  FlaskConical,
  FolderOpen,
  Globe,
  HelpCircle,
  Lightbulb,
  MessageSquareQuote,
  MoreHorizontal,
  PanelLeft,
  Paperclip,
  Package,
  Pencil,
  Plus,
  Search,
  SlidersHorizontal,
  SquareTerminal,
  ToggleRight,
  TriangleAlert,
  Users,
  X,
} from "lucide-react";
import { createPortal } from "react-dom";
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { BrandMark } from "./Wordmark";
import {
  cancelQueuedMessage,
  chatAttachmentUrl,
  createChatSession,
  deleteChatSession,
  DEMO_FIGURE_SESSION_ID,
  DEMO_LITERATURE_SESSION_ID,
  DEMO_MAIN_SESSION_ID,
  DEMO_PROJECT_ID,
  forkChatTurn,
  fmtNumber,
  getChatMessages,
  getProjectStarterPrompts,
  createRemoteSession,
  getSshHosts,
  getSkills,
  type StarterPrompt,
  interruptChat,
  listRemoteSessions,
  listChatSessions,
  reasoningFor,
  recoverChatTurn,
  reconcileReasoning,
  reconcileServiceTier,
  renameChatSession,
  retryQueuedMessage,
  respondChat,
  selectChatBranch,
  sendChatMessage,
  setChatSessionArchived,
  setChatSessionPermissionMode,
  setChatSessionPlanMode,
  type ChatImageAttachment,
  type ChatMessage,
  type ChatPart,
  type ChatPrompt,
  type ChatSession,
  type ChatTextAnnotation,
  type Harness,
  type PromptAnswer,
  type QueuedMessage,
  type RemoteSessionInfo,
  type RuntimeInfo,
  type SkillInfo,
  type SshHost,
} from "../api";
import { getLocale } from "../paraglide/runtime.js";
import { activePath, forkPositions } from "../transcriptTree";
import {
  isTurnStatusPart,
  partIsVisible,
  partsTailToolId,
  streamTailIsText,
  streamTailTool,
} from "../chatRendering";
import { onChatEvent } from "../events";
import {
  queuedRetryLabel,
  recoveryAction as parseRecoveryAction,
  recoveryTurnOptions,
  retryStatusLabel,
} from "../chatRecovery";
import {
  containsShellGlob,
  orxArgsMatch,
  orxArgv,
  shellWords,
  shellWrapperBody,
  unwrapShellBody,
} from "../orxCommand";
import { LitSourceLogo, parseOrxLit, paperUrl } from "./LitSourceLogo";
import { LitSourcesList } from "./LitSourcesPicker";
import { Md } from "./Md";
import { PlanStrip } from "./PlanStrip";
import { SETTINGS_NAV, type SettingsTab } from "./SettingsPage";
import { SkillMenu } from "./SkillMenu";
import { ComposerSkillChips, MessageWithChips } from "./SkillChips";
import { SshConfigDialog } from "./SshConfigDialog";
import { RemoteIcon } from "./RemoteIcon";
import { RemoteStatus } from "./RemoteStatus";
import {
  defaultSelection,
  HARNESS_LABELS,
  ModelPicker,
  usePopover,
  type ModelSelection,
} from "./ModelPicker";
import { ContextMeter } from "./ContextMeter";
import { renderNote } from "./agentNote";
import {
  commandsForHarness,
  effectiveCommandPlanMode,
  insertSlashCommand,
  parsePlanCommand,
  removeSlashCommand,
  slashCommandContext,
  type SlashCommandContext,
} from "../planCommand";
import { loadReadDemoSessions, markDemoSessionRead } from "../demoSessionState";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import {
  escapeMarkdownText,
  fencedCodeMarkdown,
  formatMath,
  headingMarkdown,
  isLegacyFingerprintMatch,
  inlineCodeMarkdown,
  listItemMarkdown,
  shouldRecoverLegacyMath,
  tableMarkdown,
} from "./annotationMarkdown";
import { Button, IconButton, Input, MenuItem, showAlert, Spinner } from "./ui";
import { useDialogFocus } from "./useDialogFocus";
import { PaperTitle } from "./PaperTitle";

const TOOL_LINE_CLASS_NAME = "tool-line flex-1 min-w-0 line-clamp-2 break-words text-base leading-6";
const TOOL_TARGET_LIMIT = 256;
const TOOL_TARGET_INSPECTION_LIMIT = 1_024;
const TOOL_OUTPUT_SCAN_LIMIT = 20_000;
const SELECTION_ACTION_GAP_PX = 8;
const CHAT_ANNOTATION_HIGHLIGHT_NAME = "chat-annotations";

interface ComposerAnnotation extends ChatTextAnnotation {
  id: string;
  range?: Range;
}

interface SelectionAction {
  text: string;
  range: Range;
  x: number;
  top: number;
}

function elementForNode(node: Node): Element | null {
  return node instanceof Element ? node : node.parentElement;
}

interface DomPoint {
  container: Node;
  offset: number;
}

function pointRange(point: DomPoint): Range {
  const range = document.createRange();
  range.setStart(point.container, point.offset);
  range.collapse(true);
  return range;
}

function pointBefore(left: DomPoint, right: DomPoint): boolean {
  return pointRange(left).compareBoundaryPoints(Range.START_TO_START, pointRange(right)) < 0;
}

function cloneBetween(start: DomPoint, end: DomPoint): DocumentFragment {
  const range = document.createRange();
  range.setStart(start.container, start.offset);
  range.setEnd(end.container, end.offset);
  return range.cloneContents();
}

const INLINE_MARKDOWN_TAGS = new Set(["A", "B", "CODE", "EM", "I", "STRONG"]);

function preservePartialInlineContext(range: Range, container: HTMLElement): void {
  const end = elementForNode(range.endContainer);
  if (Array.from(container.childNodes).every((node) => node.nodeType === Node.TEXT_NODE)) {
    let ancestor = elementForNode(range.startContainer);
    while (ancestor && ancestor.matches(".md *") && ancestor.contains(end)) {
      if (INLINE_MARKDOWN_TAGS.has(ancestor.tagName)) {
        const wrapper = ancestor.cloneNode(false);
        if (wrapper instanceof HTMLElement) {
          wrapper.replaceChildren(...Array.from(container.childNodes));
          container.replaceChildren(wrapper);
        }
      }
      ancestor = ancestor.parentElement;
    }
  }
  const sourcePre = elementForNode(range.startContainer)?.closest("pre");
  if (sourcePre?.contains(end)) {
    const code = sourcePre.querySelector("code")?.cloneNode(false);
    const pre = sourcePre.cloneNode(false);
    if (pre instanceof HTMLElement && code instanceof HTMLElement) {
      code.replaceChildren(...Array.from(container.childNodes));
      pre.replaceChildren(code);
      container.replaceChildren(pre);
    }
  }
}

function pruneAnnotationSelection(container: HTMLElement): void {
  container.querySelectorAll("button").forEach((button) => {
    button.replaceWith(document.createTextNode(button.textContent ?? ""));
  });
  container
    .querySelectorAll("script, style, iframe, object, embed, input, textarea, select")
    .forEach((element) => element.remove());
  container.querySelectorAll("*").forEach((element) => {
    for (const attribute of Array.from(element.attributes)) {
      if (
        attribute.name.toLowerCase().startsWith("on") ||
        attribute.name === "contenteditable" ||
        attribute.name === "tabindex"
      ) {
        element.removeAttribute(attribute.name);
      }
    }
  });
}

function selectionContent(range: Range, root: HTMLElement): HTMLDivElement {
  const container = document.createElement("div");
  const selectionEnd = { container: range.endContainer, offset: range.endOffset };
  let cursor = { container: range.startContainer, offset: range.startOffset };
  const katexNodes = Array.from(root.querySelectorAll<HTMLElement>(".katex")).filter((katex) =>
    range.intersectsNode(katex),
  );
  for (const katex of katexNodes) {
    const math = katex.closest<HTMLElement>(".katex-display") ?? katex;
    const mathRange = document.createRange();
    mathRange.selectNode(math);
    const mathStart = { container: mathRange.startContainer, offset: mathRange.startOffset };
    const mathEnd = { container: mathRange.endContainer, offset: mathRange.endOffset };
    if (pointBefore(cursor, mathStart)) container.append(cloneBetween(cursor, mathStart));
    container.append(math.cloneNode(true));
    cursor = mathEnd;
    if (!pointBefore(cursor, selectionEnd)) break;
  }
  if (katexNodes.length === 0) {
    container.append(range.cloneContents());
  } else if (pointBefore(cursor, selectionEnd)) {
    container.append(cloneBetween(cursor, selectionEnd));
  }
  preservePartialInlineContext(range, container);
  pruneAnnotationSelection(container);
  return container;
}

function markdownTable(table: HTMLElement): string {
  const rows = Array.from(table.querySelectorAll("tr")).map((row) =>
    Array.from(row.querySelectorAll(":scope > th, :scope > td"))
      .map((cell) => markdownFromSelectionNode(cell).trim().replaceAll("|", "\\|")),
  ).filter((row) => row.length > 0);
  return rows.length > 0
    ? `\n\n${tableMarkdown(rows, Boolean(table.querySelector("tr:first-child th")))}\n\n`
    : "";
}

function markdownList(list: HTMLElement): string {
  const ordered = list.tagName === "OL";
  const startAttribute = list.getAttribute("start");
  const parsedStart = startAttribute === null ? 1 : Number(startAttribute);
  let next = Number.isFinite(parsedStart) ? parsedStart : 1;
  const lines: string[] = [];
  for (const item of Array.from(list.children).filter(
    (child): child is HTMLElement => child instanceof HTMLElement && child.tagName === "LI",
  )) {
    const explicitValue = item.getAttribute("value");
    const parsedValue = explicitValue === null ? next : Number(explicitValue);
    const value = Number.isFinite(parsedValue) ? parsedValue : next;
    next = value + 1;
    const content = Array.from(item.childNodes)
      .map((child) => child instanceof HTMLElement && child.matches("UL, OL")
        ? `\n${markdownList(child).trim()}\n`
        : markdownFromSelectionNode(child))
      .join("")
      .trim();
    lines.push(listItemMarkdown(ordered ? `${value}.` : "-", content));
  }
  return `\n\n${lines.join("\n")}\n\n`;
}

function markdownFromSelectionNode(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) return escapeMarkdownText(node.textContent ?? "");
  if (!(node instanceof HTMLElement)) {
    return Array.from(node.childNodes).map(markdownFromSelectionNode).join("");
  }
  if (node.matches(".katex-display")) {
    const tex = node.querySelector("annotation[encoding='application/x-tex']")?.textContent?.trim();
    return tex ? formatMath(tex, true) : "";
  }
  if (node.matches(".katex")) {
    const tex = node.querySelector("annotation[encoding='application/x-tex']")?.textContent?.trim();
    return tex ? formatMath(tex, false) : "";
  }
  if (node.tagName === "BR") return "\n";
  if (node.tagName === "TABLE") return markdownTable(node);
  if (node.matches("UL, OL")) return markdownList(node);
  if (node.tagName === "CODE" && node.parentElement?.tagName !== "PRE") {
    return inlineCodeMarkdown(node.textContent ?? "");
  }
  if (node.tagName === "PRE") return fencedCodeMarkdown(node.textContent ?? "");
  const inner = Array.from(node.childNodes).map(markdownFromSelectionNode).join("");
  if (!inner) return "";
  if (node.matches("strong, b")) return `**${inner}**`;
  if (node.matches("em, i")) return `*${inner}*`;
  if (node.tagName === "A") {
    const href = node.getAttribute("href");
    return href ? `[${inner}](${href})` : inner;
  }
  if (node.tagName === "LI") return `${inner.trim()}\n`;
  if (node.matches("TH, TD")) return `${inner.trim()} | `;
  if (node.tagName === "TR") return `${inner.replace(/ \| $/, "")}\n`;
  if (node.tagName === "BLOCKQUOTE") {
    return `\n\n${inner.trim().split("\n").map((line) => `> ${line}`).join("\n")}\n\n`;
  }
  const heading = headingMarkdown(node.tagName, inner);
  if (heading) return `\n\n${heading}\n\n`;
  if (node.matches("P, DIV, UL, OL, TABLE")) {
    return `\n\n${inner.trim()}\n\n`;
  }
  return inner;
}

function semanticSelectionText(content: HTMLElement, fallback: string): string {
  const text = markdownFromSelectionNode(content)
    .replace(/\r\n?/g, "\n")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  return text || fallback;
}

function annotationFingerprint(text: string): string {
  return text.normalize("NFKC").replace(/[\s\u200B-\u200D\u2060\uFEFF]/g, "").toLowerCase();
}

function legacyAnnotationMarkdown(text: string, root: HTMLElement): string | undefined {
  if (!shouldRecoverLegacyMath(text)) return undefined;
  const target = annotationFingerprint(text);
  if (target.length < 8) return undefined;
  let best: { markdown: string; delta: number } | undefined;
  for (const katex of root.querySelectorAll<HTMLElement>(".msg-assistant > .md .katex")) {
    const fingerprints = [
      katex.querySelector(".katex-mathml")?.textContent,
      katex.querySelector(".katex-html")?.textContent,
      katex.textContent,
    ]
      .filter((value): value is string => Boolean(value))
      .map(annotationFingerprint);
    const match = fingerprints.find((candidate) => isLegacyFingerprintMatch(candidate, target));
    if (!match) continue;
    const tex = katex.querySelector("annotation[encoding='application/x-tex']")?.textContent?.trim();
    if (!tex) continue;
    const display = Boolean(katex.closest(".katex-display"));
    const candidate = {
      markdown: formatMath(tex, display).trim(),
      delta: Math.abs(match.length - target.length),
    };
    if (!best || candidate.delta < best.delta) best = candidate;
  }
  return best?.markdown;
}

function currentTranscriptSelection(root: HTMLElement): SelectionAction | null {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  const focusNode = selection.focusNode;
  if (!focusNode) return null;
  const commonElement = elementForNode(range.commonAncestorContainer);
  const focusElement = elementForNode(focusNode);
  if (!commonElement || !root.contains(commonElement)) return null;
  if (!focusElement || !root.contains(focusElement)) return null;
  const startElement = elementForNode(range.startContainer);
  const endElement = elementForNode(range.endContainer);
  const startMarkdown = startElement?.closest<HTMLElement>(".md");
  const endMarkdown = endElement?.closest<HTMLElement>(".md");
  const assistant = startMarkdown?.parentElement;
  if (
    !startMarkdown || !endMarkdown || !assistant?.classList.contains("msg-assistant") ||
    endMarkdown.parentElement !== assistant || startMarkdown.dataset.streaming === "true" ||
    endMarkdown.dataset.streaming === "true"
  ) return null;
  const fallbackText = selection
    .toString()
    .replace(/\r\n?/g, "\n")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  if (!fallbackText) return null;
  const content = selectionContent(range, root);

  const selectionRects = Array.from(range.getClientRects()).filter(
    (rect) => rect.width > 0 && rect.height > 0,
  );
  const firstRect = selectionRects[0] ?? range.getBoundingClientRect();
  const firstLineRects = selectionRects.filter(
    (rect) => rect.top < firstRect.bottom && rect.bottom > firstRect.top,
  );
  const lineRects = firstLineRects.length > 0 ? firstLineRects : [firstRect];
  const left = Math.min(...lineRects.map((rect) => rect.left));
  const right = Math.max(...lineRects.map((rect) => rect.right));
  const selectionTop = Math.min(...lineRects.map((rect) => rect.top));
  const selectionBottom = Math.max(...lineRects.map((rect) => rect.bottom));
  const actionHeight = 34;
  const actionHalfWidth = 74;
  const top = selectionTop >= actionHeight + SELECTION_ACTION_GAP_PX
    ? selectionTop - actionHeight - SELECTION_ACTION_GAP_PX
    : selectionBottom + SELECTION_ACTION_GAP_PX;
  return {
    text: semanticSelectionText(content, fallbackText),
    range: range.cloneRange(),
    x: Math.min(window.innerWidth - actionHalfWidth, Math.max(actionHalfWidth, left + (right - left) / 2)),
    top,
  };
}

function useTranscriptSelection(
  rootRef: React.RefObject<HTMLDivElement | null>,
  onAdd: (selection: Pick<SelectionAction, "text" | "range">) => void,
) {
  const [action, setAction] = useState<SelectionAction | null>(null);
  const selectingWithPointer = useRef(false);
  const update = useCallback(() => {
    const root = rootRef.current;
    setAction(root ? currentTranscriptSelection(root) : null);
  }, [rootRef]);

  useEffect(() => {
    let updateFrame: number | null = null;
    const selectionChanged = () => {
      if (!selectingWithPointer.current) update();
    };
    const pointerDown = (event: PointerEvent) => {
      const root = rootRef.current;
      const target = event.target;
      if (
        !event.isPrimary ||
        event.button !== 0 ||
        !root ||
        !(target instanceof Node) ||
        !root.contains(target)
      ) {
        return;
      }
      selectingWithPointer.current = true;
      setAction(null);
    };
    const pointerFinished = (event: PointerEvent) => {
      if (!event.isPrimary || !selectingWithPointer.current) return;
      selectingWithPointer.current = false;
      updateFrame = window.requestAnimationFrame(update);
    };

    document.addEventListener("selectionchange", selectionChanged);
    document.addEventListener("pointerdown", pointerDown, true);
    window.addEventListener("pointerup", pointerFinished, true);
    window.addEventListener("pointercancel", pointerFinished, true);
    return () => {
      document.removeEventListener("selectionchange", selectionChanged);
      document.removeEventListener("pointerdown", pointerDown, true);
      window.removeEventListener("pointerup", pointerFinished, true);
      window.removeEventListener("pointercancel", pointerFinished, true);
      if (updateFrame !== null) window.cancelAnimationFrame(updateFrame);
      selectingWithPointer.current = false;
    };
  }, [update]);

  useEffect(() => {
    if (!action) return;
    const dismiss = (event: MouseEvent) => {
      const target = event.target;
      if (target instanceof Element && target.closest(".chat-selection-action")) return;
      setAction(null);
    };
    document.addEventListener("mousedown", dismiss, true);
    window.addEventListener("resize", update);
    return () => {
      document.removeEventListener("mousedown", dismiss, true);
      window.removeEventListener("resize", update);
    };
  }, [action, update]);

  const add = useCallback(() => {
    if (!action) return;
    onAdd({ text: action.text, range: action.range });
    setAction(null);
    window.getSelection()?.removeAllRanges();
  }, [action, onAdd]);
  const dismiss = useCallback(() => setAction(null), []);

  return { action, add, dismiss };
}

function useAnnotationHighlights(annotations: ComposerAnnotation[]) {
  useLayoutEffect(() => {
    if (!("highlights" in CSS) || typeof Highlight === "undefined") return;
    const ranges = annotations.flatMap((annotation) =>
      annotation.range ? [annotation.range] : [],
    );
    if (ranges.length === 0) {
      CSS.highlights.delete(CHAT_ANNOTATION_HIGHLIGHT_NAME);
      return;
    }

    const highlight = new Highlight(...ranges);
    CSS.highlights.set(CHAT_ANNOTATION_HIGHLIGHT_NAME, highlight);
    return () => {
      if (CSS.highlights.get(CHAT_ANNOTATION_HIGHLIGHT_NAME) === highlight) {
        CSS.highlights.delete(CHAT_ANNOTATION_HIGHLIGHT_NAME);
      }
    };
  }, [annotations]);
}

function AnnotationPreview({ annotation }: { annotation: ComposerAnnotation }) {
  const ref = useRef<HTMLDivElement>(null);
  const [legacyMarkdown, setLegacyMarkdown] = useState<string>();
  useLayoutEffect(() => {
    const root = ref.current?.closest<HTMLElement>(".chat-thread-inner");
    setLegacyMarkdown(root ? legacyAnnotationMarkdown(annotation.text, root) : undefined);
  }, [annotation.id, annotation.text]);
  return <div ref={ref}><Md text={legacyMarkdown ?? annotation.text} /></div>;
}

function AnnotationEntries({
  annotations,
  onRemove,
}: {
  annotations: ComposerAnnotation[];
  onRemove?: (id: string) => void;
}) {
  return annotations.map((annotation, index) => (
    <div
      key={annotation.id}
      className={`annotation-item grid gap-2 py-2 px-1 [&+&]:border-t [&+&]:border-border-variant ${onRemove ? "grid-cols-[24px_minmax(0,_1fr)_28px]" : "grid-cols-[24px_minmax(0,_1fr)]"}`}
    >
      <span className="text-sm text-muted text-end">{index + 1}.</span>
      <div className="min-w-0">
        <div className="text-sm text-muted mb-1">{m.chat_panel_selected_text()}</div>
        <AnnotationPreview annotation={annotation} />
      </div>
      {onRemove && (
        <IconButton
          type="button"
          size="small"
          data-annotation-remove
          title={m.chat_panel_remove_annotation()}
          aria-label={m.a11y_remove_annotation({ number: fmtNumber(index + 1) })}
          onClick={() => onRemove(annotation.id)}
        >
          <X size={13} />
        </IconButton>
      )}
    </div>
  ));
}

function AnnotationsPopover({
  annotations,
  variant,
  onClear,
  onRemove,
}: {
  annotations: ComposerAnnotation[];
  variant: "composer" | "sent";
  onClear?: () => void;
  onRemove?: (id: string) => void;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const dialogId = useId();
  const popover = usePopover(triggerRef);
  const sent = variant === "sent";
  const closeTimer = useRef<number | null>(null);
  const openSent = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
    popover.setOpen(true);
  };
  const closeSent = () => {
    closeTimer.current = window.setTimeout(() => {
      if (!dialogRef.current?.contains(document.activeElement)) popover.setOpen(false);
    }, 160);
  };
  const toggleFromTrigger = () => {
    const opening = sent || !popover.open;
    popover.setOpen(opening);
    if (opening) window.requestAnimationFrame(() => dialogRef.current?.focus());
  };
  const removeAnnotation = (id: string) => {
    onRemove?.(id);
    window.requestAnimationFrame(() => {
      const next = dialogRef.current?.querySelector<HTMLButtonElement>("button[data-annotation-remove]");
      (next ?? dialogRef.current ?? triggerRef.current)?.focus();
    });
  };
  useEffect(
    () => () => {
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    },
    [],
  );
  return (
    <div
      className={sent ? "sent-annotations relative flex w-fit" : "composer-annotations relative flex w-fit pt-2 px-3 pb-0"}
      ref={popover.ref}
      onMouseEnter={sent ? openSent : undefined}
      onMouseLeave={sent ? closeSent : undefined}
    >
      <div className={`inline-flex items-center border border-border bg-background overflow-hidden ${sent ? "rounded-full" : "rounded-sm"}`}>
        <button
          ref={triggerRef}
          type="button"
          className={`inline-flex items-center gap-1.5 py-1 text-sm font-medium text-text [&:hover]:bg-surface ${sent ? "px-2.5" : "ps-2 pe-1.5"}`}
          aria-expanded={popover.open}
          aria-haspopup="dialog"
          aria-controls={dialogId}
          onClick={toggleFromTrigger}
        >
          <MessageSquareQuote size={sent ? 13 : 14} className="text-muted" />
          {annotations.length === 1 ? m.chat_one_annotation() : m.chat_annotation_count({ count: fmtNumber(annotations.length) })}
        </button>
        {onClear && (
          <button
            type="button"
            className="inline-flex items-center justify-center self-stretch w-6.5 text-muted border-s border-border [&:hover]:bg-surface [&:hover]:text-text"
            title={m.chat_panel_clear_annotations()}
            aria-label={m.chat_panel_clear_annotations()}
            onClick={onClear}
          >
            <X size={13} />
          </button>
        )}
      </div>
      {popover.open && (
        <div
          id={dialogId}
          ref={dialogRef}
          tabIndex={-1}
          className={`annotation-menu absolute bottom-[calc(100%_+_8px)] z-50 w-[min(440px,_calc(100vw_-_48px))] max-h-80 overflow-y-auto overscroll-contain bg-background border border-border rounded-lg shadow-popover p-2 text-start ${sent ? "end-0 after:absolute after:top-full after:start-0 after:end-0 after:h-2 after:content-['']" : "start-3"}`}
          role="dialog"
          aria-label={m.chat_panel_selected_chat_text()}
        >
          <AnnotationEntries annotations={annotations} onRemove={onRemove ? removeAnnotation : undefined} />
        </div>
      )}
    </div>
  );
}

function ComposerAnnotations(props: Omit<React.ComponentProps<typeof AnnotationsPopover>, "variant">) {
  return <AnnotationsPopover {...props} variant="composer" />;
}

const PROMPT_COLLAPSED_CLASS_NAME = [
  "prompt-collapsed text-muted text-base font-[375] my-3.5 mx-0 [&_summary]:flex",
  "[&_summary]:items-center [&_summary]:gap-2 [&_summary]:cursor-pointer",
  "[&_summary]:list-none [&_summary]:select-none [&_summary::-webkit-details-marker]:hidden",
  "[&_summary::after]:content-['›'] [&_summary::after]:text-muted",
  "[&_summary::after]:transition-transform [&_summary::after]:duration-80 [&_summary::after]:ease-standard [&[open]_summary::after]:rotate-90",
].join(" ");

const PROMPT_COLLAPSED_BODY_CLASS_NAME = [
  "prompt-collapsed-body mt-1.5 ps-3 border-s-2 border-s-border",
  "text-sm text-subtext",
].join(" ");

const PLAN_RESOLVED_CLASS_NAME = [
  "prompt-collapsed plan-resolved text-subtext my-3.5 mx-0",
  "[&_summary]:flex [&_summary]:items-center [&_summary]:gap-2 [&_summary]:w-fit [&_summary]:max-w-full",
  "[&_summary]:py-[3px] [&_summary]:px-1 [&_summary]:cursor-pointer [&_summary]:rounded-sm",
  "[&_summary]:list-none [&_summary]:select-none [&_summary:hover]:bg-surface",
  "[&_summary::-webkit-details-marker]:hidden",
  "[&_summary_.plan-chevron]:transition-transform [&_summary_.plan-chevron]:duration-120",
  "[&_summary_.plan-chevron]:ease-standard [&[open]_summary_.plan-chevron]:rotate-90",
].join(" ");

const PROMPT_HEAD_CLASS_NAME = [
  "prompt-head text-sm font-medium text-text",
  "[&_code]:font-mono [&_code]:text-sm [&_code]:text-text",
].join(" ");

const PROMPT_ACTIONS_CLASS_NAME = "prompt-actions flex flex-wrap gap-2";

// --- chat state --------------------------------------------------------------

interface ChatState {
  // Every branch of the transcript, not just the one on screen — switching
  // forks is then a pointer move rather than a refetch.
  messagesBySession: Record<string, ChatMessage[]>;
  busySessions: Set<string>;
  // Messages parked behind a running turn, per session, oldest first.
  queuedBySession: Record<string, QueuedMessage[]>;
  // Tip of the branch on screen, per session. Absent falls back to the whole
  // transcript, which is exactly right for a session that was never forked.
  activeLeafBySession: Record<string, string | null>;
}

type Action =
  | { type: "reset" }
  | {
      type: "seed";
      sessionId: string;
      messages: ChatMessage[];
      queued?: QueuedMessage[];
      activeLeafId?: string | null;
      onlyIfAbsent?: boolean;
    }
  | { type: "activeLeaf"; sessionId: string; leafId: string | null }
  // Local-only; swept by upsertMessage's LOCAL_PREFIX filter when the next
  // server message lands, and gone on reload.
  | { type: "localError"; sessionId: string; text: string }
  | { type: "upsertMessage"; sessionId: string; message: ChatMessage }
  | {
      type: "optimisticUser";
      sessionId: string;
      text: string;
      attachments: { url: string; mediaType: string; name?: string }[];
      annotations: ComposerAnnotation[];
    }
  | { type: "busy"; sessionId: string; busy: boolean }
  // `known` scopes the reseed: flags for sessions outside it (other projects —
  // busy events aren't project-filtered) are carried forward, not wiped.
  | { type: "seedBusy"; sessions: string[]; known: string[] }
  | { type: "setQueued"; sessionId: string; items: QueuedMessage[] }
  | { type: "forget"; sessionId: string };

const LOCAL_PREFIX = "local-";
const NO_MESSAGES: ChatMessage[] = [];

function upsertMessage(list: ChatMessage[], message: ChatMessage): ChatMessage[] {
  const i = list.findIndex((m) => m.id === message.id);
  if (i >= 0) {
    const next = list.slice();
    next[i] = message;
    return next;
  }
  // The server's copy of the user message replaces the optimistic local one.
  if (message.role !== "user") return [...list, message];
  return [...list.filter((m) => !m.id.startsWith(LOCAL_PREFIX)), message];
}

function reducer(state: ChatState, action: Action): ChatState {
  switch (action.type) {
    case "reset":
      return {
        messagesBySession: {},
        busySessions: new Set(),
        queuedBySession: {},
        activeLeafBySession: {},
      };
    case "seed":
      // onlyIfAbsent: recover a failed fetch without clobbering messages that
      // streamed in via SSE during it (a `message` event already created the key).
      if (action.onlyIfAbsent && action.sessionId in state.messagesBySession) return state;
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: action.messages },
        // A seed is the authoritative snapshot, so it also (re)sets the parked
        // queue — recovering it after a reload or an SSE gap.
        queuedBySession: {
          ...state.queuedBySession,
          [action.sessionId]: action.queued ?? [],
        },
        activeLeafBySession: {
          ...state.activeLeafBySession,
          [action.sessionId]: action.activeLeafId ?? null,
        },
      };
    case "upsertMessage": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      // A re-emitted message (a prompt card resolving, a streaming flush) must
      // not drag the branch pointer backwards — only a message we have not seen
      // extends the branch it arrived on.
      const known = list.some((m) => m.id === action.message.id);
      const leaf = state.activeLeafBySession[action.sessionId] ?? null;
      const replacesOptimistic =
        action.message.role === "user" && leaf !== null && leaf.startsWith(LOCAL_PREFIX);
      // A known message still moves the pointer when it hangs off the leaf. That
      // is forward-only, and it repairs a seed that raced the turn's first flush
      // and would otherwise hide the reply for the rest of the turn.
      const extendsBranch = action.message.parentId != null && action.message.parentId === leaf;
      return {
        ...state,
        messagesBySession: {
          ...state.messagesBySession,
          [action.sessionId]: upsertMessage(list, action.message),
        },
        activeLeafBySession:
          known && !replacesOptimistic && !extendsBranch
            ? state.activeLeafBySession
            : { ...state.activeLeafBySession, [action.sessionId]: action.message.id },
      };
    }
    case "localError": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      const msg: ChatMessage = {
        id: `${LOCAL_PREFIX}senderr-${Date.now()}`,
        role: "assistant",
        parts: [
          { id: "p0", type: "tool", tool: "error", state: { status: "error", error: action.text } },
        ],
        createdAt: Date.now(),
        // Sit on the branch that is showing, not at the root of a new one.
        parentId: state.activeLeafBySession[action.sessionId] ?? null,
      };
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: [...list, msg] },
        activeLeafBySession: { ...state.activeLeafBySession, [action.sessionId]: msg.id },
      };
    }
    case "activeLeaf":
      return {
        ...state,
        activeLeafBySession: {
          ...state.activeLeafBySession,
          [action.sessionId]: action.leafId,
        },
      };
    case "optimisticUser": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      const parts: ChatPart[] = action.text
        ? [{ id: "p0", type: "text", text: action.text }]
        : [];
      // Data URLs stand in until the server's copy arrives with file names.
      action.attachments.forEach((a, i) =>
        parts.push({ id: `img${i}`, type: "image", text: a.url, name: a.name }),
      );
      action.annotations.forEach((annotation, i) =>
        parts.push({
          id: `annotation${i}`,
          type: "annotation",
          text: annotation.text,
        }),
      );
      const msg: ChatMessage = {
        id: `${LOCAL_PREFIX}${Date.now()}`,
        role: "user",
        parts,
        createdAt: Date.now(),
        parentId: state.activeLeafBySession[action.sessionId] ?? null,
      };
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: [...list, msg] },
        activeLeafBySession: { ...state.activeLeafBySession, [action.sessionId]: msg.id },
      };
    }
    case "busy": {
      const busySessions = new Set(state.busySessions);
      if (action.busy) busySessions.add(action.sessionId);
      else busySessions.delete(action.sessionId);
      return { ...state, busySessions };
    }
    case "seedBusy": {
      const busySessions = new Set(action.sessions);
      const known = new Set(action.known);
      for (const id of state.busySessions) if (!known.has(id)) busySessions.add(id);
      return { ...state, busySessions };
    }
    case "setQueued": {
      return {
        ...state,
        queuedBySession: { ...state.queuedBySession, [action.sessionId]: action.items },
      };
    }
    case "forget": {
      // Deleted session: drop its transcript and busy flag so a same-id event
      // arriving late can't render stale state.
      const messagesBySession = { ...state.messagesBySession };
      delete messagesBySession[action.sessionId];
      const busySessions = new Set(state.busySessions);
      busySessions.delete(action.sessionId);
      const queuedBySession = { ...state.queuedBySession };
      delete queuedBySession[action.sessionId];
      const activeLeafBySession = { ...state.activeLeafBySession };
      delete activeLeafBySession[action.sessionId];
      return { messagesBySession, busySessions, queuedBySession, activeLeafBySession };
    }
  }
}

// --- rendering ---------------------------------------------------------------

function relTime(ts: number | undefined): string {
  if (!ts) return "";
  const seconds = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (seconds < 60) return m.relative_now_short();
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return m.relative_minutes_short({ value: fmtNumber(minutes) });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return m.relative_hours_short({ value: fmtNumber(hours) });
  return m.relative_days_short({ value: fmtNumber(Math.floor(hours / 24)) });
}

/** The last path segment, for compact display ("src/a/b.rs" → "b.rs"). */
function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

function skillNameFromPath(path: string): string | null {
  const parts = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.at(-1)?.toLowerCase() !== "skill.md") return null;
  return parts.at(-2) ?? null;
}

function nativeOrxSkillPath(tool: string, skillName: string): string | null {
  if (!/^orx-[a-z0-9]+(?:-[a-z0-9]+)*$/.test(skillName)) return null;
  if (tool === "Skill") return `.claude/skills/${skillName}/SKILL.md`;
  if (tool === "skill") return `.opencode/skills/${skillName}/SKILL.md`;
  return null;
}

type ToolActivityKind = "skill" | "read" | "search" | "edit" | "web" | "agent" | "project" | "command";

interface ToolActivity {
  kind: ToolActivityKind;
  label: string;
  progressLabel?: string;
  searchPattern?: string;
  filePath?: string;
  fileRef?: string;
  labelTarget?: string;
  litCall?: NonNullable<ReturnType<typeof parseOrxLit>>;
  runIds?: string[];
  experimentIds?: string[];
  /** Chat sessions `orx agent spawn` created in this tool call. */
  spawnedSessionIds?: string[];
}

type OpenTranscriptFile = (
  path: string,
  line: number | undefined,
  exp: string | undefined,
  ref: string | undefined,
  intent: TabOpenIntent,
) => void;
type OpenTranscriptTarget = (id: string, intent: TabOpenIntent) => void;
type OpenSubagent = (
  spawnPartId: string,
  label: string | undefined,
  intent: TabOpenIntent,
) => void;

function inputString(input: Record<string, unknown>, ...keys: string[]): string | null {
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string" && value) return value;
  }
  return null;
}

function arrayInputString(input: Record<string, unknown>, key: string, field: string): string | null {
  const values = input[key];
  if (!Array.isArray(values)) return null;
  for (const value of values) {
    if (!value || typeof value !== "object" || !(field in value)) continue;
    const candidate = value[field];
    if (typeof candidate === "string" && candidate) return candidate;
  }
  return null;
}

function inputStringArray(input: Record<string, unknown>, key: string): string[] {
  const values = input[key];
  if (!Array.isArray(values)) return [];
  const strings: string[] = [];
  for (let index = 0; index < Math.min(values.length, TOOL_TARGET_INSPECTION_LIMIT); index++) {
    if (typeof values[index] === "string") strings.push(values[index]);
    if (strings.length >= TOOL_TARGET_LIMIT) break;
  }
  return strings;
}

function exactInputStringArray(input: Record<string, unknown>, key: string): string[] | null {
  const values = input[key];
  if (!Array.isArray(values)) return null;
  const strings: string[] = [];
  for (const value of values) {
    if (typeof value !== "string") return null;
    strings.push(value);
  }
  return strings;
}

function normalizedTargetIds(...collections: string[][]): string[] {
  const ids = new Set<string>();
  const target = new RegExp(`^${RUN_TARGET_PATTERN}$`, "i");
  let inspected = 0;
  for (const collection of collections) {
    for (const value of collection) {
      if (ids.size >= TOOL_TARGET_LIMIT || inspected++ >= TOOL_TARGET_INSPECTION_LIMIT) return [...ids];
      if (target.test(value)) ids.add(value.toLowerCase());
    }
  }
  return [...ids];
}

function cleanToolError(value: string): string {
  return value
    .replace(/^Exit code \d+\s*/i, "")
    .split("\n")
    .filter((line) => !/^\s*\[orx-(?:run|experiment):[^\]]+\]\s*$/.test(line))
    .join("\n")
    .trim();
}

function editChange(input: Record<string, unknown>): { path: string; type: string | null } | null {
  const changes = input.changes;
  if (!Array.isArray(changes)) return null;
  for (const change of changes) {
    if (!change || typeof change !== "object" || !("path" in change) || typeof change.path !== "string") {
      continue;
    }
    const kind = "kind" in change ? change.kind : null;
    const type = kind && typeof kind === "object" && "type" in kind && typeof kind.type === "string"
      ? kind.type
      : null;
    return { path: change.path, type };
  }
  return null;
}

/** Codex reports shell commands as `/bin/zsh -lc <script>`. That wrapper is
 * execution plumbing, not useful transcript content. */
function meaningfulCommand(command: string): string {
  const trimmed = command.trim();
  const wrapped = trimmed.match(/^\/bin\/(?:ba|z)?sh\s+-lc\s+([\s\S]+)$/);
  let body = (wrapped?.[1] ?? trimmed).trim();
  body = unwrapShellBody(body);
  return normalizeShellBody(body);
}

function normalizeShellBody(body: string): string {
  return stripHeredocBodies(body).replace(/[\t\r ]+/g, " ").trim();
}

function heredocMarker(line: string): { delimiter: string; stripTabs: boolean } | null {
  let quote: "\"" | "'" | null = null;
  let escaped = false;
  for (let index = 0; index < line.length - 1; index++) {
    const char = line[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\" && quote !== "'") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      continue;
    }
    if (char === "\"" || char === "'") {
      quote = char;
      continue;
    }
    if (char !== "<" || line[index + 1] !== "<" || line[index + 2] === "<") continue;
    index += 2;
    const stripTabs = line[index] === "-";
    if (stripTabs) index++;
    while (/\s/.test(line[index] ?? "")) index++;
    const delimiterQuote = line[index] === "\"" || line[index] === "'" ? line[index++] : null;
    let delimiter = "";
    while (index < line.length) {
      const current = line[index];
      if (delimiterQuote ? current === delimiterQuote : /\s|[;|&]/.test(current)) break;
      delimiter += current;
      index++;
    }
    if (delimiter) return { delimiter, stripTabs };
  }
  return null;
}

function stripHeredocBodies(command: string): string {
  const lines = command.split("\n");
  const kept: string[] = [];
  let marker: { delimiter: string; stripTabs: boolean } | null = null;
  for (const line of lines) {
    if (marker) {
      const candidate = marker.stripTabs ? line.replace(/^\t+/, "") : line;
      if (candidate.trimEnd() === marker.delimiter) marker = null;
      continue;
    }
    kept.push(line);
    marker = heredocMarker(line);
  }
  return kept.join("\n");
}

function validReadTarget(value: string | undefined): string | null {
  const target = value?.replace(/[)'\"]+$/, "");
  if (
    !target
    || target === "-"
    || /^\d+$/.test(target)
    || /[$`]/.test(target)
    || containsShellGlob(target)
  ) return null;
  return target;
}

function likelyPathReadTarget(value: string | undefined): string | null {
  const target = validReadTarget(value);
  if (!target) return null;
  const name = baseName(target);
  if (
    target.includes("/")
    || target.startsWith(".")
    || /\.[A-Za-z0-9][A-Za-z0-9_-]*$/.test(name)
    || /^(?:Dockerfile|Makefile|README|LICENSE|NOTICE|CHANGELOG)$/i.test(name)
  ) {
    return target;
  }
  return null;
}

interface ShellInvocation {
  name: string;
  args: string[];
}

interface GitShowTarget {
  ref: string;
  path: string;
}

function shellInvocation(command: string): ShellInvocation | null {
  const words = shellWords(command);
  let index = 0;
  while (["do", "then", "else", "if", "while", "until"].includes(words[index])) index++;
  while (/^[A-Za-z_][A-Za-z0-9_]*=/.test(words[index] ?? "")) index++;
  if (words[index] === "command") {
    index++;
    while (words[index]?.startsWith("-")) index++;
  }
  if (words[index] === "env") {
    index++;
    while (words[index]?.startsWith("-") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[index] ?? "")) index++;
  }
  const executable = words[index];
  if (!executable) return null;
  return { name: executable.split("/").pop() ?? executable, args: words.slice(index + 1) };
}

function commandReadTarget(invocation: ShellInvocation): string | null {
  const { name, args } = invocation;

  if (name === "sed") {
    let index = 0;
    let scriptProvidedByOption = false;
    while (index < args.length && args[index].startsWith("-")) {
      const option = args[index++];
      if (option === "-e" || option === "--expression" || option === "-f" || option === "--file") {
        scriptProvidedByOption = true;
        index++;
      }
    }
    if (!scriptProvidedByOption) index++;
    for (const arg of args.slice(index)) {
      if (arg.startsWith("-")) continue;
      const target = likelyPathReadTarget(arg);
      if (target) return target;
    }
    return null;
  }

  const files: string[] = [];
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "--") {
      files.push(...args.slice(index + 1));
      break;
    }
    if (name !== "cat" && ["-n", "--lines", "-c", "--bytes", "--pid"].includes(arg)) {
      index++;
      continue;
    }
    if (arg.startsWith("-")) continue;
    files.push(arg);
  }
  return validReadTarget(files[0]);
}

function commandGitShowTarget(invocation: ShellInvocation | null): GitShowTarget | null {
  if (invocation?.name !== "git" || invocation.args[0] !== "show") return null;
  const object = invocation.args.slice(1).find((arg) => !arg.startsWith("-") && arg.includes(":"));
  if (!object) return null;
  const separator = object.indexOf(":");
  const ref = object.slice(0, separator);
  const path = object.slice(separator + 1);
  return ref && likelyPathReadTarget(path) ? { ref, path } : null;
}

function commandSearchPattern(command: string): string | null {
  const search = command.match(/\b(?:rg|grep)\b(?:\s+-[^\s]+)*\s+(?:"([^"]+)"|'([^']+)'|([^\s]+))/);
  return search?.[1] ?? search?.[2] ?? search?.[3] ?? null;
}

function resolvePosixPath(base: string, target: string): string | null {
  if (/[$`~]/.test(base) || /[$`~]/.test(target)) return null;
  const absolute = target.startsWith("/") || (!target.startsWith("/") && base.startsWith("/"));
  const parts = target.startsWith("/") ? [] : base.split("/").filter(Boolean);
  for (const part of target.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length > 0 && parts[parts.length - 1] !== "..") parts.pop();
      else if (!absolute) parts.push(part);
      continue;
    }
    parts.push(part);
  }
  const resolved = `${absolute ? "/" : ""}${parts.join("/")}`;
  return resolved || (absolute ? "/" : null);
}

function commandFilePath(
  target: string,
  segments: ShellCommandSegment[],
  segmentIndex: number,
  declaredWorkdir: string | null,
): string | null {
  if (target.startsWith("/")) return target;
  let directory = declaredWorkdir ?? "";
  for (let index = 0; index < segmentIndex; index++) {
    const invocation = shellInvocation(segments[index].raw);
    if (invocation?.name !== "cd") continue;
    const next = invocation.args.find((arg) => !arg.startsWith("-"));
    if (!next) return null;
    const resolved = resolvePosixPath(directory, next);
    if (!resolved) return null;
    directory = resolved;
  }
  return directory ? resolvePosixPath(directory, target) : target;
}

const UUID_PATTERN = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
/** Session ids as `orx agent spawn` prints them, so the spawn card can link to
 * the session it started. The id is only in the output — never the command. */
const SPAWNED_SESSION_PATTERN = new RegExp(`\\bchat_(${UUID_PATTERN})\\b`, "gi");
const RUN_TARGET_PATTERN = `(?:${UUID_PATTERN}|[0-9a-f]{8})`;

interface ShellCommandSegment {
  raw: string;
  code: string;
}

function shellCommandSegments(command: string): ShellCommandSegment[] {
  const segments: ShellCommandSegment[] = [];
  let raw = "";
  let code = "";
  let quote: "\"" | "'" | null = null;
  let escaped = false;
  const push = () => {
    if (raw.trim() || code.trim()) segments.push({ raw: raw.trim(), code: code.trim() });
    raw = "";
    code = "";
  };
  const scanSubstitution = (start: number): number => {
    let depth = 1;
    let nestedQuote: "\"" | "'" | null = null;
    let nestedEscaped = false;
    for (let index = start; index < command.length; index++) {
      const char = command[index];
      if (nestedEscaped) {
        nestedEscaped = false;
        continue;
      }
      if (char === "\\" && nestedQuote !== "'") {
        nestedEscaped = true;
        continue;
      }
      if (nestedQuote) {
        if (char === nestedQuote) nestedQuote = null;
        continue;
      }
      if (char === "\"" || char === "'") {
        nestedQuote = char;
        continue;
      }
      if (char === "(") depth++;
      if (char === ")" && --depth === 0) return index;
    }
    return command.length - 1;
  };
  const scanBackticks = (start: number): number => {
    let nestedEscaped = false;
    for (let index = start; index < command.length; index++) {
      const char = command[index];
      if (nestedEscaped) {
        nestedEscaped = false;
        continue;
      }
      if (char === "\\") {
        nestedEscaped = true;
        continue;
      }
      if (char === "`") return index;
    }
    return command.length - 1;
  };

  for (let index = 0; index < command.length; index++) {
    const char = command[index];
    if (escaped) {
      raw += char;
      if (!quote) code += char;
      escaped = false;
      continue;
    }
    if (char === "\\" && quote !== "'") {
      raw += char;
      escaped = true;
      continue;
    }
    if (quote) {
      if (quote === "\"" && char === "$" && command[index + 1] === "(") {
        const end = scanSubstitution(index + 2);
        segments.push(...shellCommandSegments(command.slice(index + 2, end)));
        raw += command.slice(index, end + 1);
        index = end;
        continue;
      }
      if (quote === "\"" && char === "`") {
        const end = scanBackticks(index + 1);
        segments.push(...shellCommandSegments(command.slice(index + 1, end)));
        raw += command.slice(index, end + 1);
        index = end;
        continue;
      }
      raw += char;
      if (char === quote) quote = null;
      continue;
    }
    if (char === "\"" || char === "'") {
      raw += char;
      quote = char;
      continue;
    }
    if (char === "`") {
      const end = scanBackticks(index + 1);
      segments.push(...shellCommandSegments(command.slice(index + 1, end)));
      raw += command.slice(index, end + 1);
      index = end;
      continue;
    }
    if (char === ";" || char === "|" || char === "&" || char === "(" || char === ")" || char === "\n") {
      push();
      continue;
    }
    raw += char;
    code += char;
  }
  push();
  return segments;
}

function orxCommandSegments(command: string, args: string): ShellCommandSegment[] {
  return shellCommandSegments(command).filter((segment) => orxArgsMatch(segment.raw, args));
}

function commandInvokesOrx(command: string, args: string): boolean {
  return orxCommandSegments(command, args).length > 0;
}

function spawnedSessionIds(output: string | undefined): string[] {
  if (!output) return [];
  const ids = new Set<string>();
  for (const match of output.slice(0, TOOL_OUTPUT_SCAN_LIMIT).matchAll(SPAWNED_SESSION_PATTERN)) {
    ids.add(match[0].toLowerCase());
    if (ids.size >= TOOL_TARGET_LIMIT) break;
  }
  return [...ids];
}

function idsFromToolOutput(output: string | undefined, resource: "runs" | "experiments"): string[] {
  if (!output) return [];
  const ids = new Set<string>();
  const boundedOutput = output.slice(0, TOOL_OUTPUT_SCAN_LIMIT);
  const patterns = resource === "runs"
    ? [
        new RegExp(`/runs/(${UUID_PATTERN})`, "gi"),
        new RegExp(`\\brun(?:_|\\s+)id:\\s*(${UUID_PATTERN})`, "gi"),
        new RegExp(`^\\s*RUN\\s+(${UUID_PATTERN})\\b`, "gim"),
        new RegExp(`={3,}\\s*(${UUID_PATTERN})\\s*={3,}`, "gi"),
      ]
    : [
        new RegExp(`/experiments/(${UUID_PATTERN})`, "gi"),
        new RegExp(`^\\s*id:\\s*(${UUID_PATTERN})`, "gim"),
        new RegExp(`={3,}\\s*(${UUID_PATTERN})\\s*={3,}`, "gi"),
      ];
  for (const pattern of patterns) {
    for (const match of boundedOutput.matchAll(pattern)) {
      ids.add(match[1]);
      if (ids.size >= TOOL_TARGET_LIMIT) return [...ids];
    }
  }
  const bareIdLine = new RegExp(`^\\s*(${UUID_PATTERN})(?:\\s|$)`, "gim");
  for (const match of boundedOutput.matchAll(bareIdLine)) {
    ids.add(match[1]);
    if (ids.size >= TOOL_TARGET_LIMIT) break;
  }
  return [...ids];
}

function invocationOffsets(command: string, invocations: ShellCommandSegment[]): Array<{ invocation: ShellCommandSegment; offset: number }> {
  let cursor = 0;
  return invocations.map((invocation) => {
    const next = command.indexOf(invocation.raw, cursor);
    const offset = next === -1 ? command.indexOf(invocation.raw) : next;
    cursor = Math.max(cursor, offset + invocation.raw.length);
    return { invocation, offset: Math.max(0, offset) };
  });
}

function assignedIdsBefore(command: string, variable: string, before: number, idPattern: string): string[] {
  const assignment = new RegExp(
    `(?:^|[\\s;])(?:export\\s+)?${variable}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s;]+))`,
    "gi",
  );
  let resolved = "";
  for (const match of command.matchAll(assignment)) {
    if ((match.index ?? 0) >= before) break;
    resolved = match[1] ?? match[2] ?? match[3] ?? "";
  }
  return [...resolved.matchAll(new RegExp(idPattern, "gi"))].map((match) => match[0]);
}

function loopIdsBefore(command: string, variable: string, before: number, idPattern: string): string[] {
  const loop = new RegExp(`\\bfor\\s+${variable}\\s+in\\s+([\\s\\S]*?)(?:;|\\n)\\s*do\\b`, "gi");
  let values = "";
  for (const match of command.matchAll(loop)) {
    const start = match.index ?? 0;
    if (start >= before) break;
    const headerEnd = start + match[0].length;
    if (headerEnd <= before && /\bdone\b/.test(command.slice(headerEnd, before))) continue;
    values = match[1];
  }
  if (/\$\(|`/.test(values)) return [];
  return [...values.matchAll(new RegExp(idPattern, "gi"))].map((match) => match[0]);
}

function commandRunIds(command: string, output?: string, preservedIds: string[] = [], legacyIds: string[] = []): string[] {
  const invocations = orxCommandSegments(command, "logs");
  const ids = new Set<string>();
  if (invocations.length === 0) {
    if (!commandInvokesOrx(command, "logs")) return [];
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "runs");
    for (const id of preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
    return normalizedTargetIds([...ids]);
  }
  let hasUnresolvedTarget = false;
  for (const { invocation, offset } of invocationOffsets(command, invocations)) {
    const argv = orxArgv(invocation.raw);
    if (argv?.[0] !== "logs") continue;
    const words = argv.slice(1);
    let target: string | null = null;
    for (let index = 0; index < words.length; index++) {
      const word = words[index];
      if (word === "--head") continue;
      if (word === "--bytes" || word === "--range") {
        index++;
        continue;
      }
      if (word.startsWith("--bytes=") || word.startsWith("--range=")) continue;
      target = word;
      break;
    }
    if (!target) {
      hasUnresolvedTarget = true;
      continue;
    }
    if (new RegExp(`^${RUN_TARGET_PATTERN}$`, "i").test(target)) {
      ids.add(target);
      continue;
    }
    const variableMatch = /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(target);
    if (!variableMatch) {
      hasUnresolvedTarget = true;
      continue;
    }
    const variable = variableMatch[1];
    const assignmentIds = assignedIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
    for (const id of assignmentIds) ids.add(id);
    const loopIds = loopIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
    for (const id of loopIds) ids.add(id);
    if (assignmentIds.length === 0 && loopIds.length === 0) hasUnresolvedTarget = true;
  }
  if (ids.size === 0 || hasUnresolvedTarget) {
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "runs");
    const fallback = preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds;
    for (const id of fallback) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
  }
  return normalizedTargetIds([...ids]);
}

function commandExperimentIds(command: string, output?: string, preservedIds: string[] = [], legacyIds: string[] = []): string[] {
  const invocations = orxCommandSegments(command, "exp\\s+(?:status|desc)");
  if (invocations.length === 0) return [];
  const ids = new Set<string>();
  let hasUnresolvedTarget = false;
  for (const { invocation, offset } of invocationOffsets(command, invocations)) {
    const argv = orxArgv(invocation.raw);
    const target = argv?.[0] === "exp" && (argv[1] === "status" || argv[1] === "desc")
      ? argv[2]
      : null;
    let resolved = false;
    if (target && new RegExp(`^${RUN_TARGET_PATTERN}$`, "i").test(target)) {
      ids.add(target);
      resolved = true;
    }
    const variableMatch = target ? /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(target) : null;
    if (variableMatch) {
      const variable = variableMatch[1];
      const assignmentIds = assignedIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
      if (assignmentIds.length > 0) {
        for (const id of assignmentIds) ids.add(id);
        resolved = true;
      }
      const loopIds = loopIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
      for (const id of loopIds) ids.add(id);
      if (loopIds.length > 0) resolved = true;
    }
    if (!resolved) hasUnresolvedTarget = true;
  }
  if (ids.size === 0 || hasUnresolvedTarget) {
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "experiments");
    const fallback = preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds;
    for (const id of fallback) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
  }
  return normalizedTargetIds([...ids]);
}

/** User-facing activity inferred from the structured tool input. Shell calls
 * get a small set of realistic recognizers; unknown commands keep their actual
 * command after the shell wrapper is removed. */
function toolActivity(part: ChatPart): ToolActivity {
  const tool = part.tool ?? "tool";
  const input = part.state?.input ?? {};
  const argumentsValue = input.arguments;
  const argumentsInput = argumentsValue && typeof argumentsValue === "object" && !Array.isArray(argumentsValue)
    ? Object.fromEntries(Object.entries(argumentsValue))
    : {};
  const normalizedInput = { ...input, ...argumentsInput };
  const rawCommand = inputString(normalizedInput, "command", "cmd");
  const commandArgv = exactInputStringArray(normalizedInput, "commandArgv");
  const toolOutput = part.state?.output || part.state?.error;
  const legacyTargetIds = normalizedTargetIds(inputStringArray(normalizedInput, "targetIds"));
  const resourceRunIds = normalizedTargetIds(inputStringArray(normalizedInput, "runTargetIds"));
  const resourceExperimentIds = normalizedTargetIds(inputStringArray(normalizedInput, "experimentTargetIds"));
  const filePath = inputString(normalizedInput, "filePath", "file_path", "notebookPath", "notebook_path", "path");
  const description = inputString(normalizedInput, "description");
  const toolSegments = tool.toLowerCase().split(/(?::|\.|__)+/);
  const baseTool = toolSegments.at(-1) ?? tool.toLowerCase();
  if (baseTool === "run" && toolSegments.includes("web")) {
    const query = arrayInputString(normalizedInput, "search_query", "q");
    const imageQuery = arrayInputString(normalizedInput, "image_query", "q");
    const pattern = arrayInputString(normalizedInput, "find", "pattern");
    if (query) return { kind: "web", label: m.activity_searched_web({ query }) };
    if (imageQuery) return { kind: "web", label: m.activity_searched_images({ query: imageQuery }) };
    if (pattern) return { kind: "web", label: m.activity_searched_web_page({ pattern }) };
    if (Array.isArray(normalizedInput.open)) return { kind: "web", label: m.chat_panel_opened_web_pages() };
    if (Array.isArray(normalizedInput.weather)) return { kind: "web", label: m.chat_panel_checked_the_weather() };
    if (Array.isArray(normalizedInput.finance)) return { kind: "web", label: m.chat_panel_checked_market_data() };
    if (Array.isArray(normalizedInput.sports)) return { kind: "web", label: m.chat_panel_checked_sports_data() };
    if (Array.isArray(normalizedInput.time)) return { kind: "web", label: m.chat_panel_checked_local_times() };
    return { kind: "web", label: m.chat_panel_browsed_the_web() };
  }
  const normalizedTool = new Map([
    ["read_file", "read"],
    ["write_file", "write"],
    ["edit_file", "edit"],
    ["exec", "bash"],
    ["exec_command", "bash"],
    ["run_command", "bash"],
    ["agent", "task"],
    ["collabagenttoolcall", "subagent"],
    ["subagentactivity", "subagent"],
  ]).get(baseTool) ?? baseTool;
  switch (normalizedTool) {
    case "bash": {
      if (!rawCommand && !commandArgv?.length) return { kind: "command", label: m.chat_panel_ran_a_command() };
      const command = meaningfulCommand(rawCommand ?? commandArgv?.join(" ") ?? "");
      const shellSegments = shellCommandSegments(command);
      let literatureInputs: Array<string | readonly string[]> = shellSegments.map((segment) => segment.raw);
      if (commandArgv?.length) {
        const wrapperBody = shellWrapperBody(commandArgv);
        literatureInputs = wrapperBody === null
          ? [commandArgv]
          : shellCommandSegments(normalizeShellBody(wrapperBody)).map((segment) => segment.raw);
      }
      let litCall: ReturnType<typeof parseOrxLit> = null;
      for (const input of literatureInputs) {
        litCall = parseOrxLit(input);
        if (litCall) break;
      }
      const hasNonLiteratureOrx = literatureInputs.some((input) => {
        const argv = orxArgv(input);
        return argv !== null && argv[0] !== "discover" && argv[0] !== "paper";
      });
      if (litCall && !hasNonLiteratureOrx) {
        const discoveryLabel = litCall.kind === "discover"
          ? {
              keyword: m.activity_searched_alphaxiv_full_text(),
              embedding: m.activity_searched_alphaxiv_semantically(),
              openalex: m.activity_searched_openalex(),
              biorxiv: m.activity_searched_biorxiv(),
            }[litCall.strategy]
          : null;
        const label = litCall.kind === "discover"
            ? litCall.query
              ? m.activity_for_query({ activity: discoveryLabel ?? m.activity_searched_literature(), query: litCall.query })
              : discoveryLabel ?? m.activity_searched_literature()
            : litCall.id ? m.activity_read_target({ target: ltr(litCall.id) }) : m.activity_read_paper();
        return { kind: litCall.kind === "paper" ? "read" : "search", label, litCall };
      }

      if (commandInvokesOrx(command, "agent\\s+spawn")) {
        return {
          kind: "agent",
          label: m.chat_panel_delegated_a_task_to_a_new_agent(),
          spawnedSessionIds: spawnedSessionIds(toolOutput),
          litCall: litCall ?? undefined,
        };
      }
      const shellInvocations = shellSegments.map((segment) => shellInvocation(segment.raw));
      const readsExperimentStatus = commandInvokesOrx(command, "exp\\s+status");
      const readsExperimentNotes = commandInvokesOrx(command, "exp\\s+desc");
      const updatesExperimentNotes = orxCommandSegments(command, "exp\\s+desc").some((segment) => {
        const argv = orxArgv(segment.raw) ?? [];
        return argv.some(
          (token) => token === "--set" || token.startsWith("--set=") || token === "--stdin",
        );
      });
      const notesLabel = updatesExperimentNotes ? m.activity_updated_experiment_notes() : m.activity_read_experiment_notes();
      const combinedLabel = updatesExperimentNotes
        ? m.activity_checked_status_updated_notes()
        : m.activity_reviewed_status_notes();
      if (commandInvokesOrx(command, "logs")) {
        const runIds = commandRunIds(command, toolOutput, resourceRunIds, legacyTargetIds);
        const label = runIds.length === 1 ? m.activity_reviewed_run_log() : m.activity_reviewed_run_logs();
        return { kind: "project", label, runIds, litCall: litCall ?? undefined };
      }
      if (commandInvokesOrx(command, "exp\\s+run")) {
        return { kind: "project", label: m.chat_panel_started_an_experiment_run(), litCall: litCall ?? undefined };
      }
      if (commandInvokesOrx(command, "exp\\s+wait")) {
        return { kind: "project", label: m.chat_panel_waited_for_an_experiment_run(), litCall: litCall ?? undefined };
      }
      if (commandInvokesOrx(command, "exp\\s+cancel")) {
        return { kind: "project", label: m.chat_panel_cancelled_an_experiment_run(), litCall: litCall ?? undefined };
      }
      const readsProject = commandInvokesOrx(command, "project\\s+view");
      if (readsProject && readsExperimentStatus && readsExperimentNotes) {
        return {
          kind: "project",
          label: combinedLabel,
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (readsProject && readsExperimentNotes) {
        return {
          kind: "project",
          label: notesLabel,
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (readsProject && readsExperimentStatus) {
        return {
          kind: "project",
          label: m.chat_panel_checked_experiment_status(),
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (readsProject) {
        return { kind: "project", label: m.chat_panel_read_project_details(), litCall: litCall ?? undefined };
      }
      if (readsExperimentStatus && readsExperimentNotes) {
        return {
          kind: "project",
          label: combinedLabel,
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (readsExperimentStatus) {
        return {
          kind: "project",
          label: m.chat_panel_checked_experiment_status(),
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (readsExperimentNotes) {
        return {
          kind: "project",
          label: notesLabel,
          experimentIds: commandExperimentIds(command, toolOutput, resourceExperimentIds, legacyTargetIds),
          litCall: litCall ?? undefined,
        };
      }
      if (commandInvokesOrx(command, "runs?")) {
        return { kind: "project", label: m.chat_panel_listed_project_runs(), litCall: litCall ?? undefined };
      }
      if (commandInvokesOrx(command, "projects")) {
        return { kind: "project", label: m.chat_panel_listed_projects(), litCall: litCall ?? undefined };
      }
      if (commandInvokesOrx(command, "compute")) {
        return { kind: "project", label: m.chat_panel_checked_compute_options(), litCall: litCall ?? undefined };
      }

      const gitShowTarget = shellInvocations
        .map(commandGitShowTarget)
        .find((target) => target != null);
      if (gitShowTarget) {
        const skillName = skillNameFromPath(gitShowTarget.path);
        return {
          kind: skillName ? "skill" : "read",
          label: skillName ? m.activity_read_skill({ name: ltr(skillName) }) : m.activity_read_target({ target: ltr(baseName(gitShowTarget.path)) }),
          filePath: gitShowTarget.path,
          fileRef: gitShowTarget.ref,
          labelTarget: skillName ? `${skillName} skill` : baseName(gitShowTarget.path),
        };
      }

      const readSegmentIndex = shellInvocations.findIndex((invocation) =>
        invocation != null && ["sed", "cat", "head", "tail"].includes(invocation.name),
      );
      const readInvocation = readSegmentIndex >= 0 ? shellInvocations[readSegmentIndex] : null;
      const readTarget = readInvocation ? commandReadTarget(readInvocation) : null;
      const readPath = readTarget
        ? commandFilePath(
            readTarget,
            shellSegments,
            readSegmentIndex,
            inputString(normalizedInput, "cwd", "workdir"),
          )
        : null;
      if (readTarget && readPath) {
        const skillName = skillNameFromPath(readPath);
        return {
          kind: skillName ? "skill" : "read",
          label: skillName ? m.activity_read_skill({ name: ltr(skillName) }) : m.activity_read_target({ target: ltr(baseName(readTarget)) }),
          filePath: readPath,
          labelTarget: skillName ? `${skillName} skill` : baseName(readTarget),
        };
      }
      if (shellInvocations.some((invocation) =>
        invocation?.name === "find"
        || invocation?.name === "ls"
        || (invocation?.name === "rg" && invocation.args.includes("--files")),
      )) {
        return { kind: "search", label: m.chat_panel_listed_files() };
      }
      const searchSegmentIndex = shellInvocations.findIndex((invocation) =>
        invocation?.name === "rg" || invocation?.name === "grep",
      );
      if (searchSegmentIndex >= 0) {
        const pattern = commandSearchPattern(shellSegments[searchSegmentIndex].raw);
        return {
          kind: "search",
          label: pattern ? m.activity_searched_code_for({ pattern: ltr(pattern) }) : m.activity_searched_code(),
          searchPattern: pattern ?? undefined,
        };
      }
      const gitInvocation = shellInvocations.find((invocation) => invocation?.name === "git");
      const gitAction = gitInvocation?.args[0];
      if (gitAction === "grep") {
        const pattern = gitInvocation?.args.slice(1).find((arg) => !arg.startsWith("-"));
        return {
          kind: "search",
          label: pattern ? m.activity_searched_code_for({ pattern: ltr(pattern) }) : m.activity_searched_code(),
          searchPattern: pattern,
        };
      }
      if (gitAction === "status") return { kind: "command", label: m.chat_panel_checked_git_status() };
      if (gitAction === "diff") return { kind: "command", label: m.chat_panel_reviewed_code_changes() };
      if (gitAction === "log") return { kind: "command", label: m.chat_panel_read_git_history() };
      const packageAction = (action: string) => shellInvocations.some((invocation) => {
        if (!invocation || !["cargo", "pnpm", "npm", "yarn"].includes(invocation.name)) return false;
        return invocation.args[0] === action || (invocation.args[0] === "run" && invocation.args[1] === action);
      });
      if (packageAction("test")) return { kind: "command", label: m.chat_panel_ran_tests() };
      if (shellInvocations.some((invocation) => invocation?.name === "tsc") || packageAction("typecheck")) {
        return { kind: "command", label: m.chat_panel_checked_types() };
      }
      if (packageAction("lint")) return { kind: "command", label: m.chat_panel_checked_code_style() };
      if (packageAction("build")) return { kind: "command", label: m.chat_panel_built_the_project() };
      return { kind: "command", label: m.activity_ran_command({ command: ltr(command) }) };
    }
    case "skill": {
      const skillName = inputString(normalizedInput, "skill", "name");
      const filePath = skillName ? nativeOrxSkillPath(tool, skillName) : null;
      return {
        kind: "skill",
        label: skillName ? m.activity_loaded_skill({ name: ltr(skillName) }) : m.activity_loaded_a_skill(),
        filePath: filePath ?? undefined,
        labelTarget: filePath && skillName ? `${skillName} skill` : undefined,
      };
    }
    case "read": {
      const target = filePath ? baseName(filePath) : null;
      const skillName = filePath ? skillNameFromPath(filePath) : null;
      if (skillName) {
        return {
          kind: "skill",
          label: m.activity_read_skill({ name: ltr(skillName) }),
          filePath: filePath ?? undefined,
          labelTarget: `${skillName} skill`,
        };
      }
      return target
        ? { kind: "read", label: m.activity_read_target({ target: ltr(target) }), filePath: filePath ?? undefined, labelTarget: target }
        : { kind: "read", label: m.chat_panel_read_a_file() };
    }
    case "edit":
    case "write":
    case "notebookedit": {
      const change = editChange(normalizedInput);
      const resolvedPath = filePath ?? change?.path ?? null;
      const target = resolvedPath ? baseName(resolvedPath) : null;
      const label = target
        ? change?.type === "add"
          ? m.activity_created_target({ target: ltr(target) })
          : change?.type === "delete"
            ? m.activity_deleted_target({ target: ltr(target) })
            : m.activity_edited_target({ target: ltr(target) })
        : null;
      return target
        ? { kind: "edit", label: label ?? m.chat_panel_edited_a_file(), filePath: resolvedPath ?? undefined, labelTarget: target }
        : { kind: "edit", label: m.chat_panel_edited_a_file() };
    }
    case "grep": {
      const pattern = inputString(normalizedInput, "pattern");
      return {
        kind: "search",
        label: pattern ? m.activity_searched_code_for({ pattern: ltr(pattern) }) : m.activity_searched_code(),
        searchPattern: pattern ?? undefined,
      };
    }
    case "glob": {
      const pattern = inputString(normalizedInput, "pattern");
      return { kind: "search", label: pattern ? m.activity_listed_files_matching({ pattern: ltr(pattern) }) : m.chat_panel_listed_files() };
    }
    case "websearch": {
      const query = inputString(normalizedInput, "query");
      const url = inputString(normalizedInput, "url");
      const pattern = inputString(normalizedInput, "pattern");
      if (query) return { kind: "web", label: m.activity_searched_web({ query }) };
      if (pattern && url) return { kind: "web", label: m.activity_searched_on_page({ pattern }) };
      if (url) return { kind: "web", label: m.activity_opened_target({ target: ltr(url) }) };
      return { kind: "web", label: description ?? m.chat_panel_browsed_the_web() };
    }
    case "webfetch": {
      const url = inputString(normalizedInput, "url");
      return { kind: "web", label: url ? m.activity_read_target({ target: ltr(url) }) : description ?? m.activity_read_web_page() };
    }
    case "task":
      // Always the task description — the row is the sub-agent's identity;
      // liveness is the shimmer, and the current step lives in its tab.
      return { kind: "agent", label: description ?? m.activity_ran_subagent() };
    case "subagent":
      return { kind: "agent", label: subagentLine(normalizedInput) };
    case "error":
      return { kind: "command", label: m.chat_panel_tool_failed() };
    case "contextcompaction":
      return {
        kind: "command",
        label: m.activity_compacted_context(),
        progressLabel: m.activity_compacting_context(),
      };
    default: {
      const detail = description ?? filePath ?? rawCommand ?? part.state?.title ?? "";
      return { kind: "command", label: detail ? `${tool}: ${detail}` : tool };
    }
  }
}

/** Readable one-liner for a Codex sub-agent spawn/activity row, from the
 * collab item fields the backend put in `state.input`. The model-assigned
 * agent name (`nickname`) is the row's identity when present — matching how
 * Claude rows show the task description — with the generic verb phrasing as
 * the fallback. */
function subagentLine(input: Record<string, unknown>): string {
  const nickname = typeof input.nickname === "string" && input.nickname
    ? input.nickname.replace(/[_-]+/g, " ")
    : "";
  // Sentence-cased for label-initial use; the bare snake_case name stays
  // lowercase when composed mid-sentence ("Sent input to audit experiments").
  const nickLabel = nickname && nickname.charAt(0).toUpperCase() + nickname.slice(1);
  if (nickLabel) return nickLabel;
  switch (typeof input.tool === "string" ? input.tool : "") {
    case "spawnAgent":
      return m.activity_spawned_agent();
    case "sendInput":
      return m.activity_sent_input_to_agent();
    case "resumeAgent":
      return m.activity_resumed_agent();
    case "wait":
      return m.activity_waiting_on_agent();
    case "closeAgent":
      return m.activity_closed_agent();
  }
  switch (typeof input.kind === "string" ? input.kind : "") {
    case "started":
      return m.activity_subagent_started();
    case "interacted":
      // Codex's cross-agent interaction marker — in practice, the agent
      // handing its report up when it finishes.
      return m.activity_agent_reported_back();
    case "interrupted":
      return m.activity_subagent_interrupted();
  }
  return m.activity_subagent();
}

function ToolActivityIcon({ activity, className = "" }: { activity: ToolActivity; className?: string }) {
  const props = { size: 16, strokeWidth: 1.75, className: "tool-kind-icon" };
  let icon: React.ReactNode = <SquareTerminal {...props} />;
  if (activity.litCall) {
    icon = <LitSourceLogo source={activity.litCall.source} size={16} className="tool-kind-icon" />;
  } else {
    switch (activity.kind) {
      case "skill":
        icon = <Blocks {...props} />;
        break;
      case "read":
      case "project":
        icon = <BookOpen {...props} />;
        break;
      case "search":
        icon = <Search {...props} />;
        break;
      case "edit":
        icon = <Pencil {...props} />;
        break;
      case "web":
        icon = <Globe {...props} />;
        break;
      case "agent":
        icon = <Users {...props} />;
        break;
      case "command":
        break;
    }
  }
  return <span className={`flex h-6 shrink-0 items-center ${className}`}>{icon}</span>;
}

function ToolTargetOverflow({
  items,
  onOpen,
  onSelect,
  targetType,
}: {
  items: Array<{ id: string; label: string }>;
  onOpen?: OpenTranscriptTarget;
  onSelect?: (id: string) => void;
  targetType: string;
}) {
  const [open, setOpen] = useState(false);
  const revealRef = useRef<HTMLSpanElement>(null);
  const focusReveal = useRef(false);

  useEffect(() => {
    if (!open || !focusReveal.current) return;
    focusReveal.current = false;
    revealRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [open]);

  return (
    <span className="tool-target-overflow inline">
      {open && (
        <span className="tool-target-reveal" ref={revealRef}>
          {items.map((item, index) => (
            <span key={item.id}>
              {index > 0 && ", "}
              {onOpen || onSelect ? (
                <button
                  className="tool-target"
                  {...(onOpen
                    ? tabOpenGestureHandlers<HTMLButtonElement>(
                        (intent) => onOpen(item.id, intent),
                        { stopPropagation: true },
                      )
                    : {
                        onClick: (event: ReactMouseEvent<HTMLButtonElement>) => {
                          event.stopPropagation();
                          onSelect?.(item.id);
                        },
                      })}
                >
                  {item.label}
                </button>
              ) : (
                <span>{item.label}</span>
              )}
            </span>
          ))}
        </span>
      )}
      {open && ", "}
      <button
        className="tool-target-more"
        aria-expanded={open}
        aria-label={open ? m.a11y_hide_additional({ target: targetType }) : m.a11y_show_more_targets({ count: fmtNumber(items.length), target: targetType })}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          focusReveal.current = !open && event.detail === 0;
          setOpen((value) => !value);
        }}
      >
        {open ? m.common_show_less() : m.common_more_count({ count: fmtNumber(items.length) })}
      </button>
    </span>
  );
}

function ToolActivityLabel({
  activity,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
}: {
  activity: ToolActivity;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
}) {
  if (activity.searchPattern) {
    return activity.label;
  }
  if (activity.litCall?.kind === "paper" && activity.litCall.id) {
    return (
      <a
        className="tool-target"
        href={paperUrl(activity.litCall.source, activity.litCall.id)}
        target="_blank"
        rel="noopener noreferrer"
      >
        {activity.label}
        <ArrowUpRight className="inline ms-1 opacity-50" size={13} aria-hidden="true" />
      </a>
    );
  }
  if (activity.filePath && activity.labelTarget && onOpenFile) {
    const filePath = activity.filePath;
    return (
      <span
        className="tool-target"
        role="button"
        tabIndex={0}
        {...tabOpenGestureHandlers<HTMLSpanElement>((intent) =>
          onOpenFile(filePath, undefined, undefined, activity.fileRef, intent),
        { stopPropagation: true })}
      >
        {activity.label}
      </span>
    );
  }
  if (activity.spawnedSessionIds?.length && onOpenSpawnedSession) {
    const sessionIds = activity.spawnedSessionIds;
    const visibleSessionIds = sessionIds.slice(0, 3);
    const hiddenSessions = sessionIds.slice(visibleSessionIds.length).map((sessionId, index) => ({
      id: sessionId,
      label: m.chat_agent_number({ number: fmtNumber(visibleSessionIds.length + index + 1) }),
    }));
    return (
      <>
        {activity.label}{" — "}
        {visibleSessionIds.map((sessionId, index) => (
          <span key={sessionId}>
            {index > 0 && ", "}
            <button
              className="tool-target"
              title={m.chat_panel_open_the_session_this_agent_spawned()}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onOpenSpawnedSession(sessionId);
              }}
            >
              {m.chat_agent_number({ number: fmtNumber(index + 1) })}
            </button>
          </span>
        ))}
        {hiddenSessions.length > 0 && (
          <>
            {", "}
            <ToolTargetOverflow
              items={hiddenSessions}
              onSelect={onOpenSpawnedSession}
              targetType={m.chat_agent_sessions()}
           />
          </>
        )}
      </>
    );
  }
  if (activity.runIds?.length) {
    const runIds = runExperimentName
      ? activity.runIds.filter((runId) => Boolean(runExperimentName(runId)))
      : activity.runIds;
    if (runIds.length === 0) return activity.label;
    const visibleRunIds = runIds.slice(0, 3);
    const hiddenRuns = runIds.slice(visibleRunIds.length).map((runId) => ({
      id: runId,
      label: runExperimentName?.(runId) || m.tree_experiment(),
    }));
    return (
      <>
        {activity.label}{" — "}
        {visibleRunIds.map((runId, index) => (
          <span key={runId}>
            {index > 0 && ", "}
            {onOpenRun ? (
              <button
                className="tool-target"
                title={m.a11y_open_logs_for_run({ run: ltr(runId) })}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                  onOpenRun(runId, intent),
                { stopPropagation: true })}
              >
                {runExperimentName?.(runId) || m.tree_experiment()}
              </button>
            ) : (
              <span>{runExperimentName?.(runId) || m.tree_experiment()}</span>
            )}
          </span>
        ))}
        {hiddenRuns.length > 0 && (
          <>
            {", "}
            <ToolTargetOverflow items={hiddenRuns} onOpen={onOpenRun} targetType={m.chat_run_logs()} />
          </>
        )}
      </>
    );
  }
  if (activity.experimentIds?.length) {
    const experimentIds = experimentName
      ? activity.experimentIds.filter((experimentId) => Boolean(experimentName(experimentId)))
      : activity.experimentIds;
    if (experimentIds.length === 0) return activity.label;
    const visibleExperimentIds = experimentIds.slice(0, 3);
    const hiddenExperiments = experimentIds.slice(visibleExperimentIds.length).map((experimentId) => ({
      id: experimentId,
      label: experimentName?.(experimentId) || m.tree_experiment(),
    }));
    return (
      <>
        {activity.label}{" — "}{visibleExperimentIds.map((experimentId, index) => (
          <span key={experimentId}>
            {index > 0 && ", "}
            {onOpenExperiment ? (
              <button
                className="tool-target"
                title={m.a11y_open_experiment({ name: experimentName?.(experimentId) || ltr(experimentId) })}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
                  onOpenExperiment(experimentId, intent),
                { stopPropagation: true })}
              >
                {experimentName?.(experimentId) || m.tree_experiment()}
              </button>
            ) : (
              <span>{experimentName?.(experimentId) || m.tree_experiment()}</span>
            )}
          </span>
        ))}
        {hiddenExperiments.length > 0 && (
          <>
            {", "}
            <ToolTargetOverflow items={hiddenExperiments} onOpen={onOpenExperiment} targetType={m.chat_experiments()} />
          </>
        )}
      </>
    );
  }
  return activity.label;
}

function activityInProgress(activity: ToolActivity): ToolActivity {
  const label = activity.progressLabel ?? {
    skill: m.activity_loading(),
    read: m.activity_reading(),
    search: m.activity_searching(),
    edit: m.activity_editing(),
    project: m.activity_reviewing(),
    web: m.activity_browsing(),
    agent: m.activity_delegating(),
    command: m.activity_running(),
  }[activity.kind];
  return { ...activity, label };
}

function permissionActivityLabel(tool: string | undefined, input: Record<string, unknown> | undefined): string {
  const activity = toolActivity({
    id: "permission-preview",
    type: "tool",
    tool,
    state: { status: "running", input },
  });
  return {
    skill: m.activity_load(),
    read: m.activity_read(),
    search: m.activity_search(),
    edit: m.activity_edit(),
    project: m.activity_review(),
    web: m.activity_browse(),
    agent: m.activity_delegate(),
    command: m.activity_run(),
  }[activity.kind];
}

function emptyToolInput(input: unknown): boolean {
  if (input == null) return true;
  return typeof input === "object" && !Array.isArray(input) && Object.keys(input).length === 0;
}

const TOOL_LABEL_DWELL_MS = 250;

/** Hold each in-progress tool label on screen for a minimum dwell before
 * swapping to the next, so a burst of sub-second calls reads as a steady
 * sequence instead of a flicker. Activation and deactivation are immediate —
 * only label→label swaps are paced — and the swap always lands on the latest
 * activity, skipping intermediates that expired within one dwell. */
function useDwelledActivity(activity: ToolActivity | null, provisional: boolean): ToolActivity | null {
  const [shown, setShown] = useState<ToolActivity | null>(activity);
  const shownAt = useRef(Date.now());
  const latest = useRef(activity);
  useEffect(() => {
    // Updated in the effect (not during render) so discarded concurrent
    // renders can't leak into the pending swap's timer.
    latest.current = activity;
    if (activity?.label === shown?.label) return;
    // A provisional activity (its call not yet classifiable) never replaces a
    // real label — hold the previous one until the call resolves. It still
    // paints when there is nothing better to show.
    if (provisional && activity != null && shown != null) return;
    if (activity == null || shown == null) {
      shownAt.current = Date.now();
      setShown(activity);
      return;
    }
    const remaining = TOOL_LABEL_DWELL_MS - (Date.now() - shownAt.current);
    if (remaining <= 0) {
      shownAt.current = Date.now();
      setShown(activity);
      return;
    }
    const timeout = window.setTimeout(() => {
      shownAt.current = Date.now();
      setShown(latest.current);
    }, remaining);
    return () => window.clearTimeout(timeout);
  }, [activity, shown, provisional]);
  // Same-label activities pass through fresh: the dwell paces label swaps
  // only, so metadata that resolves later (run ids, file refs) isn't held
  // back with a stale copy.
  return activity != null && activity.label === shown?.label ? activity : shown;
}

const TOOL_TAIL_SHIMMER_DELAY_MS = 160;
function useDelayedToolShimmer(active: boolean): boolean {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (!active) {
      setVisible(false);
      return;
    }
    const timeout = window.setTimeout(
      () => setVisible(true),
      TOOL_TAIL_SHIMMER_DELAY_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [active]);

  return active && visible;
}

function groupIconActivity(activities: ToolActivity[]): ToolActivity {
  const priority: ToolActivityKind[] = ["skill", "read", "search", "edit", "project", "web", "command", "agent"];
  for (const kind of priority) {
    const activity = activities.find((candidate) => candidate.kind === kind);
    if (activity) return activity;
  }
  return activities[0] ?? { kind: "command", label: m.chat_panel_used_tools() };
}

interface SquashedToolPart {
  part: ChatPart;
  activity: ToolActivity;
  count: number;
}

function squashableToolPartKey(part: ChatPart, activity: ToolActivity): string | null {
  if (part.state?.status !== "completed") return null;
  return JSON.stringify([
    activity.kind,
    activity.label,
    activity.filePath ?? null,
    activity.fileRef ?? null,
    activity.litCall?.kind === "paper" ? activity.litCall.id ?? null : null,
    activity.runIds ?? null,
    activity.experimentIds ?? null,
    activity.spawnedSessionIds ?? null,
  ]);
}

function squashToolParts(parts: ChatPart[]): SquashedToolPart[] {
  const squashed: SquashedToolPart[] = [];
  let previousKey: string | null = null;
  for (const part of parts) {
    const activity = toolActivity(part);
    const key = squashableToolPartKey(part, activity);
    const previous = squashed[squashed.length - 1];
    if (key && previous && previousKey === key) {
      previous.count++;
    } else {
      squashed.push({ part, activity, count: 1 });
    }
    previousKey = key;
  }
  return squashed;
}

function TurnStatusRow({
  part,
  busy,
  recovering,
  onRecover,
}: {
  part: ChatPart;
  busy: boolean;
  recovering: boolean;
  onRecover?: (turnId: string, action: "retry" | "continue") => void;
}) {
  const input = part.state?.input;
  const nextRetryAt = input?.nextRetryAt ?? null;
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (typeof nextRetryAt !== "number") return;
    setNow(Date.now());
    if (nextRetryAt <= Date.now()) return;
    const timer = window.setInterval(() => {
      const current = Date.now();
      setNow(current);
      if (current >= nextRetryAt) window.clearInterval(timer);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [nextRetryAt]);
  if (part.id === "turn-retry") {
    const label = retryStatusLabel(input ?? {}, now);
    return (
      <div className="turn-retry-row flex items-center gap-2 py-1 px-1 text-sm text-subtext">
        <Spinner />
        <span>{label}</span>
      </div>
    );
  }
  const action = parseRecoveryAction(input?.recoveryAction);
  const turnId = input?.turnId;
  if ((action !== "retry" && action !== "continue") || !turnId) return null;
  const label = action === "retry" ? m.app_retry() : m.chat_continue();
  const errorMessage = cleanToolError(part.state?.error || m.chat_turn_incomplete());
  return (
    <div className="turn-recovery-row flex items-center justify-between gap-2 py-1.5 px-2.5 border border-border rounded-md bg-background">
      <span className="min-w-0 truncate text-sm text-accent-red" title={errorMessage}>
        {errorMessage}
      </span>
      <Button
        type="button"
        size="small"
        disabled={busy || recovering}
        onClick={() => onRecover?.(turnId, action)}
      >
        {recovering ? m.chat_starting() : label}
      </Button>
    </div>
  );
}

/** Routine successful calls are static activity rows. Only failures disclose
 * raw command/output, because that detail is useful for diagnosis. */
function ToolRow({
  part,
  repeatCount = 1,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
}: {
  part: ChatPart;
  repeatCount?: number;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
}) {
  const state = part.state;
  const activity = toolActivity(part);
  const failed = state?.status === "error";
  const errorMessage = cleanToolError(state?.error || state?.output || "");
  const hasDetail = failed && Boolean(errorMessage);
  const [detailOpen, setDetailOpen] = useState(false);
  const detailId = `tool-error-${part.id.replace(/[^A-Za-z0-9_-]/g, "-")}`;
  const line = (
    <>
      {failed && <span className="sr-only">{m.chat_panel_failed()} </span>}
      {failed ? (
        <span className="flex h-6 shrink-0 items-center text-accent-red">
          <CircleX size={16} strokeWidth={1.75} className="tool-kind-icon" aria-hidden="true" />
        </span>
      ) : (
        <ToolActivityIcon activity={activity} className="text-muted" />
      )}
      <span className={`${TOOL_LINE_CLASS_NAME} ${failed ? "text-accent-red" : "text-subtext"}`}>
        <ToolActivityLabel
          activity={activity}
          onOpenFile={onOpenFile}
          onOpenRun={onOpenRun}
          onOpenSpawnedSession={onOpenSpawnedSession}
          runExperimentName={runExperimentName}
          onOpenExperiment={onOpenExperiment}
          experimentName={experimentName}
       />
        {repeatCount > 1 && (
          <span className="tool-repeat-count ms-1 text-muted font-normal" title={m.a11y_identical_calls({ count: fmtNumber(repeatCount) })}>
            ×{repeatCount}
          </span>
        )}
      </span>
    </>
  );

  if (!hasDetail) {
    return <div className="tool-row flex items-start gap-2 min-w-0 py-[3px] px-1">{line}</div>;
  }

  return (
    <div className="tool-row tool-row-error flex flex-col min-w-0">
      <div className="flex items-start gap-2 w-fit max-w-full py-[3px] px-1 min-w-0 rounded-sm">
        {line}
        <button
          type="button"
          className="tool-row-detail-toggle inline-flex h-6 shrink-0 items-center justify-center p-0.5 rounded-sm cursor-pointer hover:bg-surface"
          aria-expanded={detailOpen}
          aria-controls={detailId}
          aria-label={detailOpen ? m.a11y_hide_error_details({ activity: activity.label }) : m.a11y_show_error_details({ activity: activity.label })}
          onClick={() => setDetailOpen((current) => !current)}
        >
          <ChevronRight size={16} className={`text-accent-red transition-transform duration-120 ease-standard ${detailOpen ? "rotate-90" : ""}`} />
        </button>
      </div>
      {detailOpen && (
        <div className="tool-detail mt-1 me-0 mb-1 ms-6" id={detailId}>
          <div className="tool-output py-1.5 px-2.5 font-mono text-xs text-subtext whitespace-pre-wrap wrap-anywhere max-h-65 overflow-y-auto bg-background border border-border-variant rounded-sm">
            {errorMessage.slice(0, 20000)}
          </div>
        </div>
      )}
    </div>
  );
}

/** Consecutive calls render as one Codex-style activity group: a readable
 * aggregate description and a collapsible list of semantic rows. */
function ToolGroup({
  parts,
  pendingTail,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
}: {
  parts: ChatPart[];
  pendingTail?: boolean;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
}) {
  const [open, setOpen] = useState(false);
  const displayParts = squashToolParts(parts);
  const activities = displayParts.map(({ activity }) => activity);
  const tail = pendingTail ? displayParts.at(-1) : undefined;
  const tailPart = tail?.part;
  const tailActivity = tail?.activity;
  const rawPending = tailPart?.state?.status !== "error"
    ? (tailActivity && activityInProgress(tailActivity)) ?? null
    : null;
  // Hold the prior label while a call lacks input, unless its name provides a
  // specific progress label already.
  const tailUnclassified = !!tailPart && tailPart.state?.status === "running" &&
    !rawPending?.progressLabel &&
    (emptyToolInput(tailPart.state?.input) || (rawPending?.kind === "command" && !inputString(tailPart.state?.input ?? {}, "command", "cmd")));
  const pendingActivity = useDwelledActivity(rawPending, tailUnclassified);
  const shimmering = useDelayedToolShimmer(pendingActivity != null);
  const iconActivity = pendingActivity ?? groupIconActivity(activities);
  const summaryLabel = pendingActivity
    ? pendingActivity.label
    : m.chat_panel_used_tools();
  if (parts.length === 1) {
    if (pendingActivity) {
      return (
        <div className="tool-group my-3.5 mx-0">
          <div className="tool-row flex items-start gap-2 min-w-0 py-[3px] px-1 text-base leading-6 text-subtext">
            <ToolActivityIcon activity={pendingActivity} className={shimmering ? "tool-running-shimmer-icon" : "text-muted"} />
            <span
              className={`${shimmering ? "tool-running-shimmer" : ""} min-w-0 line-clamp-2 break-words`}
              title={summaryLabel}
            >
              <ToolActivityLabel
                activity={pendingActivity}
                onOpenFile={onOpenFile}
                onOpenRun={onOpenRun}
                onOpenSpawnedSession={onOpenSpawnedSession}
                runExperimentName={runExperimentName}
                onOpenExperiment={onOpenExperiment}
                experimentName={experimentName}
             />
            </span>
          </div>
        </div>
      );
    }
    return (
      <div className="tool-group my-3.5 mx-0">
        <ToolRow
          part={parts[0]}
          onOpenFile={onOpenFile}
          onOpenRun={onOpenRun}
          onOpenSpawnedSession={onOpenSpawnedSession}
          runExperimentName={runExperimentName}
          onOpenExperiment={onOpenExperiment}
          experimentName={experimentName}
       />
      </div>
    );
  }

  return (
    <div className="tool-group my-3.5 mx-0">
      <div className="tool-group-summary flex items-start gap-2 w-fit max-w-full py-[3px] px-1 text-base leading-6 text-subtext text-start">
        <ToolActivityIcon activity={iconActivity} className={shimmering ? "tool-running-shimmer-icon" : "text-muted"} />
        {pendingActivity ? (
          <span
            className={`tool-group-label min-w-0 line-clamp-2 break-words ${shimmering ? "tool-running-shimmer" : ""}`}
            title={summaryLabel}
          >
            <ToolActivityLabel
              activity={pendingActivity}
              onOpenFile={onOpenFile}
              onOpenRun={onOpenRun}
              onOpenSpawnedSession={onOpenSpawnedSession}
              runExperimentName={runExperimentName}
              onOpenExperiment={onOpenExperiment}
              experimentName={experimentName}
           />
          </span>
        ) : (
          <button
            type="button"
            className="tool-group-label min-w-0 whitespace-normal break-words cursor-pointer text-start"
            onClick={() => setOpen((value) => !value)}
            aria-expanded={open}
          >
            {summaryLabel}
          </button>
        )}
        <button
          type="button"
          className="tool-group-chevron-button inline-flex h-6 shrink-0 items-center justify-center p-px cursor-pointer rounded-sm"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          aria-label={open ? m.chat_collapse_tool_activity() : m.chat_expand_tool_activity()}
        >
          <ChevronRight size={16} className={`tool-chevron text-muted transition-[transform,color] duration-120 ease-standard [&.open]:rotate-90 ${open ? "open" : ""}`} />
        </button>
      </div>
      <div
        className={`tool-group-disclosure ${open ? "open" : ""}`}
        aria-hidden={!open}
        inert={!open}
      >
        <div className="tool-group-disclosure-inner">
          <div className="tool-group-rows flex flex-col gap-px mt-0.5 me-0 mb-1 ms-6">
            {displayParts.map(({ part, count }) => (
              <ToolRow
                key={part.id}
                part={part}
                repeatCount={count}
                onOpenFile={onOpenFile}
                onOpenRun={onOpenRun}
                onOpenSpawnedSession={onOpenSpawnedSession}
                runExperimentName={runExperimentName}
                onOpenExperiment={onOpenExperiment}
                experimentName={experimentName}
             />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Interactive card for a plan / permission / question prompt. Approving (or
 * answering) resumes the session. Once resolved, cards mirror Claude Code:
 * a permission leaves no trace, a plan collapses to an expandable
 * "Proposed plan" row, a question collapses to a compact record of the
 * chosen answer — all inline at the card's chronological position. */
function PromptCard({
  part,
  onRespond,
  onOpenFile,
  onOpenPlan,
}: {
  part: ChatPart;
  onRespond?: (answer: PromptAnswer) => void;
  onOpenFile?: OpenTranscriptFile;
  onOpenPlan?: (plan: string, promptId: string, intent: TabOpenIntent) => void;
}) {
  const p = part.prompt as ChatPrompt;
  const [picked, setPicked] = useState<string[]>([]);
  // Read-only host (no onRespond): actions disabled or hidden, card visible.
  const done = !onRespond;

  const respond = (answer: Omit<PromptAnswer, "promptId">) =>
    onRespond?.({ promptId: part.id, ...answer });

  // Resolved rendering, keyed off `resolved` alone (`done` also covers
  // read-only hosts, where an *unresolved* card must stay visible).
  if (p.resolved) {
    if (p.kind === "permission") return null;
    if (p.kind === "plan") {
      const outcome = p.approved === true
        ? { label: m.chat_panel_plan_approved(), icon: Check, iconClass: "text-accent-green" }
        : p.approved === false && p.note
          ? { label: m.chat_panel_plan_revision_requested(), icon: Pencil, iconClass: "text-accent-amber" }
          : p.approved === false
            ? { label: m.chat_panel_plan_rejected(), icon: X, iconClass: "text-accent-red" }
            : { label: m.chat_panel_plan_resolved(), icon: FileText, iconClass: "text-muted" };
      const OutcomeIcon = outcome.icon;
      return (
        <details className={PLAN_RESOLVED_CLASS_NAME}>
          <summary>
            <span className="plan-resolved-label text-base font-[375] wrap-anywhere">
              {p.synthesized ? m.chat_plan() : m.chat_proposed_plan()}
            </span>
            <OutcomeIcon size={17} strokeWidth={1.8} className={`shrink-0 ${outcome.iconClass}`} />
            <span className="plan-resolved-label prompt-outcome text-base font-[375] wrap-anywhere">{outcome.label}</span>
            <ChevronRight size={12} className="plan-chevron shrink-0 text-muted" />
          </summary>
          <div className={`${PROMPT_COLLAPSED_BODY_CLASS_NAME} ms-6`}>
            <Md text={p.plan ?? ""} onOpenFile={onOpenFile} />
            {p.note && <div className="prompt-collapsed-note mt-1.5 italic">{p.note}</div>}
          </div>
        </details>
      );
    }
    // question — one line: header/question + what was chosen (or the typed
    // custom answer). No echo at all (stale-resolved): neutral "Resolved",
    // matching the plan row.
    const chosen = (p.answers ?? []).join(", ") || p.note || "";
    const annotations: ComposerAnnotation[] = (p.annotations ?? []).map((annotation, index) => ({
      id: `${part.id}-annotation-${index}`,
      text: annotation.text,
    }));
    return (
      <div className="flex flex-col items-end gap-1.5">
        {annotations.length > 0 && <AnnotationsPopover annotations={annotations} variant="sent" />}
        <details className={PROMPT_COLLAPSED_CLASS_NAME}>
          <summary>
            <span className="prompt-collapsed-title font-[375] wrap-anywhere">{p.header || p.question || m.chat_question()}</span>
            <span className={`prompt-outcome font-[375] text-subtext wrap-anywhere [&.approved]:text-accent-green [&.chosen]:text-accent-green [&.approved::before]:content-['✓_'] [&.chosen::before]:content-['✓_'] [&.revised]:text-accent-amber [&.rejected]:text-accent-amber ${chosen ? "chosen" : ""}`}>{chosen || m.chat_resolved()}</span>
          </summary>
          <div className={PROMPT_COLLAPSED_BODY_CLASS_NAME}>
            {/* The summary title already shows the question when there's no header. */}
            {p.header && p.question && <div className="prompt-q text-base font-semibold leading-normal text-text">{p.question}</div>}
            {(p.options ?? []).length > 0 && (
              <ul className="prompt-collapsed-options mt-1.5 mx-0 mb-0 ps-4.5 [&_.sel]:text-text [&_.sel]:font-medium">
                {(p.options ?? []).map((o) => (
                  <li key={o.label} className={p.answers?.includes(o.label) ? "sel" : ""}>
                    {o.label}
                  </li>
                ))}
              </ul>
            )}
            {/* A note-only answer is already the summary outcome — don't echo it twice. */}
            {p.note && p.note !== chosen && <div className="prompt-collapsed-note mt-1.5 italic">{p.note}</div>}
          </div>
        </details>
      </div>
    );
  }

  if (p.kind === "plan") {
    // With a plan-strip host (onOpenPlan), the docked strip owns the approval
    // actions and the full plan lives in the right pane — the inline card is a
    // compact, clamped in-transcript record. Without one, it keeps its own
    // buttons (approving leaves plan mode; resumeMode values are
    // harness-agnostic permission-mode wire ids).
    const docked = !!onOpenPlan;
    return (
      <div className={`prompt-card my-2 mx-0 py-3 px-3.5 border border-border border-s-[3px] border-s-border rounded-sm bg-surface flex flex-col gap-[9px] [&.plan]:border-s-accent-blue [&.permission]:border-s-accent-amber [&.question]:border-s-accent-purple [&.readonly]:opacity-60 plan ${done ? "readonly" : ""}`}>
        <div className="prompt-head text-base font-semibold text-text">
          {p.synthesized ? m.chat_plan_ready() : m.chat_proposed_plan()}
        </div>
        <div className={`prompt-plan text-base leading-[1.6] text-text max-h-85 overflow-y-auto [&.clamped]:max-h-[9.5em] [&.clamped]:overflow-hidden [&.clamped]:relative [&.clamped::after]:content-[''] [&.clamped::after]:absolute [&.clamped::after]:inset-x-0 [&.clamped::after]:bottom-0 [&.clamped::after]:top-auto [&.clamped::after]:h-8.5 [&.clamped::after]:bg-[linear-gradient(to_bottom,_transparent,_var(--surface))] [&.clamped::after]:pointer-events-none ${docked ? "clamped" : ""}`}>
          <Md text={p.plan ?? ""} onOpenFile={onOpenFile} />
        </div>
        {docked && (
          <button
            className="prompt-plan-open self-start border-0 bg-transparent text-accent-blue text-sm p-0 cursor-pointer [&:hover]:underline"
            {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
              onOpenPlan(p.plan ?? "", part.id, intent),
            )}
          >
            {m.chat_panel_view_full_plan()}
          </button>
        )}
        {/* Strip-less fallback (unreachable in the main app — App always
            provides onOpenPlan): same action semantics as the strip. */}
        {!done && !docked && (
          <div className={PROMPT_ACTIONS_CLASS_NAME}>
            <Button size="small" variant="primary" onClick={() => respond({ approve: true, resumeMode: "auto" })}>
              {m.chat_panel_accept_and_auto_mode()}
            </Button>
            <Button size="small" onClick={() => respond({ approve: true, resumeMode: "bypassPermissions" })}>
              {m.chat_panel_accept_and_bypass_all()}
            </Button>
            <Button size="small" onClick={() => respond({ approve: false })}>
              {m.chat_panel_reject()}
            </Button>
          </div>
        )}
      </div>
    );
  }

  if (p.kind === "permission") {
    const toolInput = p.toolInput ?? {};
    const summary =
      inputString(toolInput, "command", "cmd", "filePath", "file_path", "path") ||
      "";
    // Codex approval cards ship a human-readable reason (and fileChange cards
    // carry nothing else) — show it so the user knows what they're granting.
    const reason =
      (typeof p.toolInput?.reason === "string" && p.toolInput.reason) || "";
    const description = inputString(toolInput, "description") || "";
    const explanation = reason || description || permissionActivityLabel(p.tool, toolInput);
    const headingId = `permission-heading-${part.id}`;
    return (
      <div
        className={`prompt-card permission my-3 w-full max-w-2xl overflow-hidden rounded-md border border-border bg-background shadow-hairline [&.readonly]:opacity-60 ${done ? "readonly" : ""}`}
        role="group"
        aria-labelledby={headingId}
      >
        <div className="flex items-center gap-2.5 px-3.5 pt-3 pb-0">
          <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-accent-amber-subtle text-accent-amber">
            <TriangleAlert size={15} strokeWidth={1.8} aria-hidden="true" />
          </span>
          <span id={headingId} className="text-base font-semibold text-text">{m.chat_panel_approval_required()}</span>
        </div>
        <div className="flex flex-col gap-3 px-3.5 py-3">
          <div className="prompt-sub text-base font-normal leading-normal text-text wrap-anywhere">{explanation}</div>
          {summary && (
            <code className="prompt-command block max-h-36 overflow-auto whitespace-pre-wrap wrap-anywhere rounded-md border border-border-variant bg-surface px-3 py-2 font-mono text-sm leading-relaxed text-text">
              {summary}
            </code>
          )}
          {!done && (
            // No resumeMode: the harness picks the right one for an approval.
            // Claude resumes under `bypassPermissions` (the only mode that grants a
            // blocked tool — acceptEdits would re-deny Bash); inline harnesses
            // (opencode) reply once/reject keyed off `approve`. Deny denies either way.
            <div className="prompt-actions flex items-center justify-end gap-2 pt-0.5">
              <Button
                size="small"
                variant="ghost"
                onClick={() => respond({ approve: false })}
              >
                {m.chat_panel_deny()}
              </Button>
              <Button
                size="small"
                variant="primary"
                onClick={() => respond({ approve: true })}
              >
                {m.chat_panel_allow()}
              </Button>
            </div>
          )}
        </div>
      </div>
    );
  }

  // question
  const toggle = (label: string) =>
    setPicked((cur) =>
      p.multiSelect
        ? cur.includes(label)
          ? cur.filter((l) => l !== label)
          : [...cur, label]
        : [label],
    );
  return (
    <div className={`prompt-card my-2 mx-0 py-3 px-3.5 border border-border border-s-[3px] border-s-border rounded-sm bg-surface flex flex-col gap-[9px] [&.plan]:border-s-accent-blue [&.permission]:border-s-accent-amber [&.question]:border-s-accent-purple [&.readonly]:opacity-60 question ${done ? "readonly" : ""}`}>
      {p.header && <div className={PROMPT_HEAD_CLASS_NAME}>{p.header}</div>}
      {p.question && <div className="prompt-q text-base font-semibold leading-normal text-text">{p.question}</div>}
      <div className="prompt-options flex flex-col gap-1.5">
        {(p.options ?? []).map((o) => {
          const sel = picked.includes(o.label);
          return (
            <button
              key={o.label}
              className={`prompt-option flex flex-col items-start gap-0.5 w-full py-2 px-[11px] text-start border border-border rounded-sm bg-background text-text cursor-pointer transition-[border-color,background] duration-80 ease-standard [&:hover:not(:disabled)]:border-border-strong [&:hover:not(:disabled)]:bg-surface [&.sel]:border-primary [&.sel]:bg-primary-subtle [&:disabled]:cursor-default ${sel ? "sel" : ""}`}
              disabled={done}
              onClick={() => (done ? undefined : p.multiSelect ? toggle(o.label) : respond({ answers: [o.label] }))}
            >
              <span className="prompt-option-label block text-sm font-medium">{o.label}</span>
              {o.description && <span className="prompt-option-desc block text-sm font-normal leading-[1.45] text-subtext">{o.description}</span>}
            </button>
          );
        })}
      </div>
      {p.multiSelect && !done && (
        <div className={PROMPT_ACTIONS_CLASS_NAME}>
          <Button
            size="small"
            variant="primary"
            disabled={picked.length === 0}
            onClick={() => respond({ answers: picked })}
          >
            {m.chat_panel_submit()}
          </Button>
        </div>
      )}
    </div>
  );
}

/** Whether a message renders anything once resolved-permission cards vanish —
 * a bridge permission card rides its own message, so resolving it leaves the
 * message empty and it must drop out of the transcript entirely. */
function messageHasVisibleContent(m: ChatMessage, activePermissionId?: string | null): boolean {
  if (m.role === "user") return true;
  return m.parts.some((part) => partIsVisible(part, activePermissionId));
}

/** Memoized: streaming re-broadcasts the whole updated message up to ~13x/sec, and
 * `upsertMessage` preserves object identity for every untouched message — so
 * only the message actually being streamed re-renders (and re-parses its
 * markdown/KaTeX), not the entire transcript. Callback props must stay
 * referentially stable for this to hold (see the useCallback/useMemo wiring
 * in ChatPanel). `Transcript` below adds a second boundary for the other hot
 * path — composer keystrokes re-render ChatPanel itself, and the transcript
 * memo stops those from touching the rows at all. */
/** Resolve an `image` (attachment) part into what the transcript renders: a
 * source URL, whether it's a PDF (file chip vs inline image), and a name. */
function attachmentPartView(p: ChatPart): { src: string; isPdf: boolean; name: string } {
  const raw = p.text ?? "";
  const src = raw.startsWith("data:") ? raw : chatAttachmentUrl(raw);
  // Server file names embed the original after a `__` marker; optimistic parts
  // carry the real name on the part instead (no server file yet).
  const derived = raw.startsWith("data:")
    ? ""
    : raw.includes("__")
      ? raw.slice(raw.indexOf("__") + 2)
      : raw;
  const name = p.name || derived || "attachment";
  const isPdf =
    raw.startsWith("data:application/pdf") || /\.pdf$/i.test(name) || /\.pdf$/i.test(raw);
  return { src, isPdf, name };
}

/** The pager stays visible once a prompt has more than one version — hiding it
 * would leave no sign that the other versions exist. */
function ForkControls({
  count,
  index,
  prevId,
  nextId,
  onSelect,
  pagerDisabled,
  onEdit,
  editDisabled,
}: {
  count: number;
  index: number;
  prevId?: string;
  nextId?: string;
  onSelect: (leafId: string) => void;
  pagerDisabled: boolean;
  onEdit: () => void;
  editDisabled: boolean;
}) {
  const many = count > 1;
  return (
    <div
      className={`fork-controls flex items-center gap-0.5 transition-opacity duration-80 ease-standard ${
        many ? "opacity-100" : "opacity-0 group-hover/turn:opacity-100 group-focus-within/turn:opacity-100"
      }`}
    >
      {many && (
        <>
          <IconButton size="small"
            title={m.chat_panel_previous_version()}
            aria-label={m.chat_panel_previous_version()}
            disabled={pagerDisabled || !prevId}
            onClick={() => prevId && onSelect(prevId)}
          >
            <ChevronLeft size={14} />
          </IconButton>
          <span className="fork-count text-xs text-subtext tabular-nums select-none">
            {index + 1}/{count}
          </span>
          <IconButton size="small"
            title={m.chat_panel_next_version()}
            aria-label={m.chat_panel_next_version()}
            disabled={pagerDisabled || !nextId}
            onClick={() => nextId && onSelect(nextId)}
          >
            <ChevronRight size={14} />
          </IconButton>
        </>
      )}
      <IconButton size="small"
        title={m.chat_panel_edit_and_re_send()}
        aria-label={m.chat_panel_edit_and_re_send()}
        disabled={editDisabled}
        onClick={onEdit}
      >
        <Pencil size={13} />
      </IconButton>
    </div>
  );
}

const Message = memo(function Message({
  message,
  activePermissionId,
  pendingTailToolId,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onRespond,
  onOpenPlan,
  onOpenSubagent,
  busy = false,
  recoveringTurnId,
  onRecover,
  skills,
  predictTextTail = false,
  forkCount,
  forkIndex = 0,
  forkPrevId,
  forkNextId,
  forkDisabled,
  branchDisabled,
  onFork,
  onSelectFork,
}: {
  message: ChatMessage;
  activePermissionId: string | null;
  pendingTailToolId?: string | null;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
  onRespond?: (answer: PromptAnswer) => void;
  /** Open a plan's full markdown in the right pane (plan cards/strip). */
  onOpenPlan?: (plan: string, promptId: string, intent: TabOpenIntent) => void;
  /** Open a sub-agent's transcript in the right pane (spawn-row "view"). */
  onOpenSubagent?: OpenSubagent;
  busy?: boolean;
  recoveringTurnId?: string | null;
  onRecover?: (turnId: string, action: "retry" | "continue") => void;
  /** Known slash-skills, for rendering a `/name` token as a command chip. */
  skills?: SkillInfo[];
  predictTextTail?: boolean;
  /** Set only on a user message, the one bearer of the fork controls. */
  forkCount?: number;
  forkIndex?: number;
  forkPrevId?: string;
  forkNextId?: string;
  forkDisabled: boolean;
  /** Paging between forks is a read-only pointer move, so it stays available
   * when editing does not (an unavailable harness still has branches to read). */
  branchDisabled: boolean;
  onFork: (messageId: string, text: string) => void;
  onSelectFork: (leafId: string) => void;
}) {
  useLocale();
  // Editing re-asks as a new fork rather than rewriting history, so the original
  // stays reachable through the pager.
  const [editDraft, setEditDraft] = useState<string | null>(null);
  if (message.role === "user") {
    const text = message.parts
      .filter((p) => p.type === "text")
      .map((p) => p.text ?? "")
      .join("\n");
    // Known `/command` tokens render as the chips the composer showed, where
    // they were typed. Unknown commands (or skills removed since) stay plain text.
    const isCommand = (name: string) => !!skills?.some((s) => s.name === name);
    // Optimistic parts carry a data URL; server parts carry a file name.
    const attachments = message.parts
      .filter((p) => p.type === "image" && p.text)
      .map(attachmentPartView);
    const images = attachments.filter((a) => !a.isPdf);
    const files = attachments.filter((a) => a.isPdf);
    const annotations: ComposerAnnotation[] = message.parts
      .filter((part) => part.type === "annotation" && part.text)
      .map((part) => ({
        id: part.id,
        text: part.text ?? "",
      }));
    if (editDraft !== null) {
      const submit = () => {
        const next = editDraft.trim();
        // Closing the editor on a send that cannot run would drop the edit with
        // nothing to show for it.
        if (!next || forkDisabled) return;
        setEditDraft(null);
        onFork(message.id, next);
      };
      return (
        <div className="msg-user-group self-end flex w-full max-w-[88%] flex-col items-end gap-1.5">
          <div className="msg-user-edit w-full bg-surface rounded-[16px] py-2.5 px-[15px] flex flex-col gap-2">
            <textarea
              dir="auto"
              className="w-full bg-transparent text-base text-text resize-none outline-none field-sizing-content min-h-16"
              aria-label={m.chat_panel_edit_message()}
              value={editDraft}
              autoFocus
              onChange={(e) => setEditDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setEditDraft(null);
                } else if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  submit();
                }
              }}
           />
            <div className={`${PROMPT_ACTIONS_CLASS_NAME} justify-end`}>
              <Button size="small" onClick={() => setEditDraft(null)}>
                {m.chat_panel_cancel()}
              </Button>
              <Button
                size="small"
                variant="primary"
                onClick={submit}
                disabled={forkDisabled || !editDraft.trim()}
              >
                {m.chat_panel_send()}
              </Button>
            </div>
          </div>
        </div>
      );
    }
    return (
      <div className="msg-user-group group/turn self-end flex max-w-[88%] flex-col items-end gap-1.5">
        {annotations.length > 0 && (
          <AnnotationsPopover annotations={annotations} variant="sent" />
        )}
        <div dir="auto" className="msg-user max-w-full bg-surface rounded-[16px] py-2.5 px-[15px] text-base whitespace-pre-wrap wrap-anywhere [&_.skill-chip]:me-0.5 [&_.skill-chip]:align-baseline">
          <MessageWithChips text={text} isCommand={isCommand} />
          {images.length > 0 && (
            <div className="msg-images flex flex-wrap gap-1.5 mt-2 [&_img]:max-w-55 [&_img]:max-h-40 [&_img]:border [&_img]:border-border-variant [&_img]:rounded-xs [&_img]:block">
              {images.map((a, i) => (
                <a key={i} href={a.src} target="_blank" rel="noreferrer">
                  <img src={a.src} alt={m.chat_attachment_alt()} />
                </a>
              ))}
            </div>
          )}
          {files.length > 0 && (
            <div className="msg-files flex flex-wrap gap-1.5 mt-2">
              {files.map((a, i) => (
                <a key={i} className="msg-file inline-flex items-center gap-1.5 max-w-60 py-1.5 px-2.5 border border-border-variant rounded-sm text-text no-underline [&:hover]:border-text [&_span]:overflow-hidden [&_span]:text-ellipsis [&_span]:whitespace-nowrap" href={a.src} target="_blank" rel="noreferrer">
                  <FileText size={15} />
                  <span>{a.name}</span>
                </a>
              ))}
            </div>
          )}
        </div>
        {forkCount !== undefined && (
          <ForkControls
            count={forkCount}
            index={forkIndex}
            prevId={forkPrevId}
            nextId={forkNextId}
            onSelect={onSelectFork}
            pagerDisabled={branchDisabled}
            onEdit={() => setEditDraft(text)}
            editDisabled={forkDisabled}
         />
        )}
      </div>
    );
  }
  const turnStatus = message.parts.find(isTurnStatusPart);
  const regularParts = turnStatus
    ? message.parts.filter((part) => part !== turnStatus)
    : message.parts;
  return (
    <div className="msg-assistant group/turn text-base leading-[1.62] text-text min-w-0">
      {renderParts(regularParts, {
        activePermissionId,
        pendingTailToolId,
        onOpenFile,
        onOpenRun,
        onOpenSpawnedSession,
        runExperimentName,
        onOpenExperiment,
        experimentName,
        onRespond,
        onOpenPlan,
        onOpenSubagent,
        predictTextTail,
      })}
      {turnStatus && (
        <TurnStatusRow
          part={turnStatus}
          busy={busy}
          recovering={recoveringTurnId === turnStatus.state?.input?.turnId}
          onRecover={onRecover}
       />
      )}
    </div>
  );
});

/** Shared assistant-parts renderer, reused for a message body and (recursively)
 * for a sub-agent's nested transcript. Coalesces consecutive tool parts into one
 * collapsed group (Claude-desktop style); text / prompt parts break a run and
 * render inline, while reasoning parts are never rendered. A sub-agent spawn
 * part (tool `subagent`) also breaks the run and renders as its own nested
 * block. */
function renderParts(
  parts: ChatPart[],
  opts: {
    activePermissionId?: string | null;
    pendingTailToolId?: string | null;
    onOpenFile?: OpenTranscriptFile;
    onOpenRun?: OpenTranscriptTarget;
    onOpenSpawnedSession?: (sessionId: string) => void;
    runExperimentName?: (runId: string) => string;
    onOpenExperiment?: OpenTranscriptTarget;
    experimentName?: (experimentId: string) => string;
    onRespond?: (answer: PromptAnswer) => void;
    onOpenPlan?: (plan: string, promptId: string, intent: TabOpenIntent) => void;
    onOpenSubagent?: OpenSubagent;
    predictTextTail?: boolean;
  },
): React.ReactNode[] {
  const {
    activePermissionId,
    pendingTailToolId,
    onOpenFile,
    onOpenRun,
    onOpenSpawnedSession,
    runExperimentName,
    onOpenExperiment,
    experimentName,
    onRespond,
    onOpenPlan,
    onOpenSubagent,
    predictTextTail = false,
  } = opts;
  // A steer never becomes the tail — the streaming caret belongs on the
  // assistant text it interrupted.
  const visibleTail = parts
    .filter((part) => part.type !== "steer" && partIsVisible(part, activePermissionId))
    .at(-1);
  const rendered: React.ReactNode[] = [];
  let toolRun: ChatPart[] = [];
  const flushTools = () => {
    if (toolRun.length === 0) return;
    rendered.push(
      <ToolGroup
        key={`tg-${toolRun[0].id}`}
        parts={toolRun}
        pendingTail={toolRun.some((part) => part.id === pendingTailToolId)}
        onOpenFile={onOpenFile}
        onOpenRun={onOpenRun}
        onOpenSpawnedSession={onOpenSpawnedSession}
        runExperimentName={runExperimentName}
        onOpenExperiment={onOpenExperiment}
        experimentName={experimentName}
     />,
    );
    toolRun = [];
  };
  for (const part of parts) {
    // A part that renders nothing must not break a tool run either — e.g. the
    // empty reasoning parts encrypted-thinking models produced (stored
    // transcripts predating the ingest-side skip still carry them), or a
    // resolved permission card. Without this, each invisible part splits
    // consecutive tools into single-row groups.
    if (!partIsVisible(part, activePermissionId)) continue;
    // A sub-agent spawn part streams its own transcript in `children` — render
    // it as a standalone nested block, not folded into a tool run. Codex tags
    // its rows `subagent`; Claude's `Task`/`Agent` and OpenCode's `task` are
    // spawn tools by name (a prose-only agent may have zero children yet still
    // carry a final report); anything else with children streamed into it is a
    // spawn too.
    if (
      part.type === "tool" &&
      (isSpawnTool(part.tool) || (part.children?.length ?? 0) > 0)
    ) {
      flushTools();
      rendered.push(
        <SubagentBlock
          key={part.id}
          part={part}
          // A spawn row runs in parallel with whatever streams after it, so its
          // shimmer follows its own status while the turn is live — the shared
          // tail-tool id only ever points at one row and would freeze the rest.
          pendingTail={(predictTextTail && part.state?.status === "running") || part.id === pendingTailToolId}
          onOpenSubagent={onOpenSubagent}
       />,
      );
      continue;
    }
    if (part.type === "tool") {
      toolRun.push(part);
      continue;
    }
    flushTools();
    // The visibility skip above guarantees text parts here are non-empty and
    // that reasoning parts never reach this point.
    if (part.type === "text")
      rendered.push(
        <Md
          key={part.id}
          text={part.text!}
          onOpenFile={onOpenFile}
          onOpenRun={onOpenRun}
          predict={predictTextTail && part.id === visibleTail?.id}
       />,
      );
    else if (part.type === "steer")
      // A message the user sent into this turn while it ran — the same bubble a
      // user message gets, sitting where the agent received it.
      rendered.push(
        <div
          key={part.id}
          dir="auto"
          role="note"
          aria-label={m.chat_panel_you_mid_task()}
          className="msg-steer my-2 ms-auto w-fit max-w-[88%] bg-surface rounded-[16px] py-2.5 px-[15px] text-base whitespace-pre-wrap wrap-anywhere"
        >
          {part.text}
        </div>,
      );
    else if (part.type === "prompt" && part.prompt)
      rendered.push(
        <PromptCard
          key={part.id}
          part={part}
          onRespond={onRespond}
          onOpenFile={onOpenFile}
          onOpenPlan={onOpenPlan}
       />,
      );
  }
  flushTools();
  return rendered;
}

/** A sub-agent spawn row's display title — what its tab is named. */
export function spawnRowTitle(part: ChatPart): string {
  return toolActivity(part).label;
}

/** Whether a tool name is a sub-agent spawn: codex tags rows `subagent`,
 * Claude spawns via `Task`/`Agent`, OpenCode via `task`. */
function isSpawnTool(tool: string | undefined): boolean {
  const name = (tool ?? "").toLowerCase();
  return name === "subagent" || name === "task" || name === "agent";
}

/** The spawn tool result that stands in for a prose-less sub-agent transcript
 * (a sync Claude agent's final report is delivered as the tool output). The
 * async-launch acknowledgement is internal metadata, not a report — newly
 * stored parts omit it, and the prefix guard covers older transcripts. */
function spawnFinalReport(part: ChatPart): string {
  const output = part.state?.status === "completed" ? (part.state?.output ?? "") : "";
  return output.startsWith("Async agent launched") ? "" : output;
}

/** Find a part by id anywhere in a parts tree (depth-first). Used by the
 * end-pane sub-agent tab to locate a spawn part across a session's messages. */
export function findPartById(parts: ChatPart[], id: string): ChatPart | null {
  for (const part of parts) {
    if (part.id === id) return part;
    const nested = part.children && findPartById(part.children, id);
    if (nested) return nested;
  }
  return null;
}

/** The sub-agent's transcript, rendered standalone in the end-pane tab (the
 * only place the transcript is shown — the inline row just opens this). Reuses
 * `renderParts`, so nested sub-agents are themselves click-to-open rows. */
export function SubagentTranscript({
  spawn,
  onOpenFile,
  onOpenRun,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onOpenSubagent,
}: {
  spawn: ChatPart;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
  onOpenSubagent?: OpenSubagent;
}) {
  const parts = spawn.children ?? [];
  const running = spawn.state?.status === "running";
  const errored = spawn.state?.status === "error";
  const errorMessage = errored
    ? cleanToolError(spawn.state?.error || spawn.state?.output || "")
    : "";
  // Gate the empty state on what actually renders, not the raw part count — a
  // stored transcript of nothing but invisible parts must still read as empty.
  const rendered = renderParts(parts, {
    onOpenFile,
    onOpenRun,
    runExperimentName,
    onOpenExperiment,
    experimentName,
    onOpenSubagent,
    predictTextTail: running,
    // Same contract as the main transcript's streamTailTool: while the
    // sub-agent runs, its tail tool (completed or not) keeps the group lit.
    pendingTailToolId: running ? partsTailToolId(parts) : null,
  });
  // Claude Code forwards a sub-agent's tool activity but never its text/thinking
  // blocks — the final report only exists as the spawn tool's result. When the
  // streamed transcript carries no prose of its own, close it with that report.
  const hasProseChild = parts.some((p) => p.type === "text" && !!p.text);
  const finalReport = hasProseChild ? "" : spawnFinalReport(spawn);
  return (
    <div className="msg-assistant text-base leading-[1.62] text-text min-w-0">
      {errored && <span className="sr-only">{m.chat_panel_failed()} </span>}
      {errorMessage && (
        <div className="tool-output py-1.5 px-2.5 font-mono text-xs text-subtext whitespace-pre-wrap wrap-anywhere max-h-65 overflow-y-auto bg-background border border-border-variant rounded-sm">
          {errorMessage.slice(0, 20000)}
        </div>
      )}
      {rendered.length === 0 && !finalReport && !errorMessage ? (
        <div className="subagent-empty py-[3px] px-1 text-sm text-muted">{running ? m.chat_working() : m.chat_no_activity()}</div>
      ) : (
        <>
          {rendered}
          {finalReport && <Md text={finalReport} onOpenFile={onOpenFile} onOpenRun={onOpenRun} />}
        </>
      )}
    </div>
  );
}

/** A Codex/Claude/OpenCode sub-agent spawn row: a one-liner (icon + label)
 * whether the agent is running (shimmer) or done. Click-to-open — the
 * transcript shows in a right-side panel tab, never inline — when there is
 * anything to open (streamed children, a final report, or an error); a pure
 * interaction marker renders as an inert status line instead. */
function SubagentBlock({
  part,
  pendingTail,
  onOpenSubagent,
}: {
  part: ChatPart;
  pendingTail?: boolean;
  onOpenSubagent?: OpenSubagent;
}) {
  const errored = part.state?.status === "error";
  const errorMessage = cleanToolError(part.state?.error || part.state?.output || "");
  const activity = pendingTail && !errored
    ? activityInProgress(toolActivity(part))
    : toolActivity(part);
  const shimmering = useDelayedToolShimmer(Boolean(pendingTail && !errored));
  // Openable when there is anything to show in the tab: streamed children, a
  // final report standing in for them, or an error. Only a pure interaction
  // marker (codex's "reported back" rows) is inert.
  const inert = (part.children?.length ?? 0) === 0 && !errored && !spawnFinalReport(part);
  const line = (
    <>
      {errored && <span className="sr-only">{m.chat_panel_failed()} </span>}
      {errored ? (
        <span className="flex h-6 shrink-0 items-center text-accent-red">
          <CircleX size={16} strokeWidth={1.75} className="subagent-icon" aria-hidden="true" />
        </span>
      ) : (
        <ToolActivityIcon activity={activity} className={`subagent-icon ${shimmering ? "tool-running-shimmer-icon" : "text-muted"}`} />
      )}
      {/* Spawn rows read as activity, not prose — gray like the tool rows
          around them. */}
      <span className={`${TOOL_LINE_CLASS_NAME} ${shimmering ? "tool-running-shimmer" : errored ? "text-accent-red" : "text-subtext"}`}>{activity.label}</span>
    </>
  );
  // Only a row that actually owns a transcript is click-to-open. Codex's
  // interaction markers (and a spawn row before any activity arrived) have no
  // children — offering a transcript there opens an empty pane.
  if (inert) {
    return (
      <div className="subagent-row flex items-start gap-2 w-full my-3.5 mx-0 py-[3px] px-1 text-text text-base text-start rounded-sm">
        {line}
      </div>
    );
  }
  return (
    <button
      className="subagent-row flex items-start gap-2 w-full my-3.5 mx-0 py-[3px] px-1 cursor-pointer text-text text-base text-start rounded-sm [&:hover:not(:disabled)]:bg-surface [&:disabled]:cursor-default"
      title={errored && errorMessage ? errorMessage : m.chat_open_subagent_transcript()}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
        onOpenSubagent?.(part.id, activity.label, intent),
      )}
      disabled={!onOpenSubagent}
    >
      {line}
      <span className="subagent-row-chevron flex h-6 shrink-0 items-center text-muted">
        <ChevronRight size={12} />
      </span>
    </button>
  );
}

/** Memoized transcript: composer keystrokes re-render ChatPanel (draft state
 * lives there), and this boundary keeps them from re-allocating N Message
 * elements and running N memo comparisons. Every prop passed here must stay
 * referentially stable across keystrokes (memoized/useCallback, never inline)
 * or the boundary silently breaks — with that held, typing costs one shallow
 * compare instead of O(messages) work. */
interface AnnouncedToolState {
  status: string;
  part: ChatPart;
}

function latestToolStates(messages: ChatMessage[]): { messageId: string; states: Map<string, AnnouncedToolState> } {
  const states = new Map<string, AnnouncedToolState>();
  let message: ChatMessage | undefined;
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index].role !== "assistant") continue;
    message = messages[index];
    break;
  }
  if (!message) return { messageId: "", states };
  const visit = (parts: ChatPart[], parent: string) => {
    for (const part of parts) {
      const path = `${parent}/${part.id}`;
      if (part.type === "tool" && part.state?.status) {
        states.set(path, { status: part.state.status, part });
      }
      if (part.children?.length) visit(part.children, path);
    }
  };
  visit(message.parts, message.id);
  return { messageId: message.id, states };
}

function firstPendingPermission(messages: ChatMessage[]): { id: string; path: string; label: string } | null {
  const visit = (parts: ChatPart[], parent: string): { id: string; path: string; label: string } | null => {
    for (const part of parts) {
      const prompt = part.prompt;
      if (part.type === "prompt" && prompt?.kind === "permission" && !prompt.resolved) {
        const toolInput = prompt.toolInput ?? {};
        const reason = inputString(toolInput, "reason", "description");
        const label = reason || permissionActivityLabel(prompt.tool, toolInput);
        return { id: part.id, path: `${parent}/${part.id}`, label };
      }
      if (part.children?.length) {
        const nested = visit(part.children, `${parent}/${part.id}`);
        if (nested) return nested;
      }
    }
    return null;
  };
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    const pending = visit(message.parts, message.id);
    if (pending) return pending;
  }
  return null;
}

interface TranscriptAnnouncement {
  text: string;
  sequence: number;
}

function useTranscriptAnnouncement(messages: ChatMessage[]): TranscriptAnnouncement {
  const [announcement, setAnnouncement] = useState<TranscriptAnnouncement>({ text: "", sequence: 0 });
  const previous = useRef<{
    transcript: string;
    messageId: string;
    states: Map<string, AnnouncedToolState>;
    permissionPath: string | null;
  } | null>(null);
  useEffect(() => {
    const transcript = messages[0]?.id ?? "";
    const { messageId, states } = latestToolStates(messages);
    const pendingPermission = firstPendingPermission(messages);
    if (!previous.current || previous.current.transcript !== transcript) {
      previous.current = { transcript, messageId, states, permissionPath: pendingPermission?.path ?? null };
      setAnnouncement((current) => ({
        text: pendingPermission ? m.announcement_approval_required({ label: autoDir(pendingPermission.label) }) : "",
        sequence: current.sequence + 1,
      }));
      return;
    }
    const previousStates = previous.current.messageId === messageId ? previous.current.states : new Map<string, AnnouncedToolState>();
    const previousPermissionPath = previous.current.permissionPath;
    const changes = [...states].filter(([path, state]) => previousStates.get(path)?.status !== state.status);
    previous.current = { transcript, messageId, states, permissionPath: pendingPermission?.path ?? null };
    if (pendingPermission && pendingPermission.path !== previousPermissionPath) {
      setAnnouncement((current) => ({
        text: m.announcement_approval_required({ label: autoDir(pendingPermission.label) }),
        sequence: current.sequence + 1,
      }));
      return;
    }
    const turnStatus = changes.find(([, state]) => isTurnStatusPart(state.part))?.[1].part;
    if (turnStatus?.id === "turn-recovery") {
      const action = parseRecoveryAction(turnStatus.state?.input?.recoveryAction);
      setAnnouncement((current) => ({
        text: `${m.announcement_turn_incomplete()}${action ? ` ${action === "retry" ? m.announcement_retry_available() : m.announcement_continue_available()}` : ""}`,
        sequence: current.sequence + 1,
      }));
      return;
    }
    if (turnStatus?.id === "turn-retry") {
      setAnnouncement((current) => ({
        text: m.announcement_cli_retrying(),
        sequence: current.sequence + 1,
      }));
      return;
    }
    const failures = changes.filter(([, state]) => state.status === "error");
    if (failures.length > 0) {
      const labels = failures.slice(0, 2).map(([, state]) => toolActivity(state.part).label).join(", ");
      setAnnouncement((current) => ({
        text: failures.length === 1
          ? m.announcement_tool_failed({ labels })
          : m.announcement_tools_failed({ count: fmtNumber(failures.length), labels }),
        sequence: current.sequence + 1,
      }));
      return;
    }
    const running = changes.filter(([, state]) => state.status === "running");
    if (running.length > 0) {
      const part = running.at(-1)?.[1].part;
      setAnnouncement((current) => ({
        text: part ? activityInProgress(toolActivity(part)).label : m.announcement_running_tool(),
        sequence: current.sequence + 1,
      }));
      return;
    }
    if (changes.some(([, state]) => state.status === "completed")) {
      setAnnouncement((current) => ({ text: m.announcement_tool_completed(), sequence: current.sequence + 1 }));
    }
  }, [messages]);
  return announcement;
}

const Transcript = memo(function Transcript({
  messages,
  allMessages,
  canFork,
  onFork,
  onSelectFork,
  busy,
  onOpenFile,
  onOpenRun,
  onOpenSpawnedSession,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onRespond,
  onOpenPlan,
  onOpenSubagent,
  recoveringTurnId,
  onRecover,
  skills,
}: {
  /** The branch on screen, oldest first. */
  messages: ChatMessage[];
  /** Every branch, for counting the forks of each turn. */
  allMessages: ChatMessage[];
  /** False greys out the edit control (busy turn, harness not ready). */
  canFork: boolean;
  onFork: (messageId: string, text: string) => void;
  onSelectFork: (leafId: string) => void;
  busy: boolean;
  onOpenFile?: OpenTranscriptFile;
  onOpenRun?: OpenTranscriptTarget;
  onOpenSpawnedSession?: (sessionId: string) => void;
  runExperimentName?: (runId: string) => string;
  onOpenExperiment?: OpenTranscriptTarget;
  experimentName?: (experimentId: string) => string;
  onRespond?: (answer: PromptAnswer) => void;
  onOpenPlan?: (plan: string, promptId: string, intent: TabOpenIntent) => void;
  onOpenSubagent?: OpenSubagent;
  recoveringTurnId?: string | null;
  onRecover?: (turnId: string, action: "retry" | "continue") => void;
  skills?: SkillInfo[];
}) {
  useLocale();
  const activePermissionId = firstPendingPermission(messages)?.id ?? null;
  const visibleMessages = useMemo(
    () => messages.filter((message) => messageHasVisibleContent(message, activePermissionId)),
    [messages, activePermissionId],
  );
  // forkPositions indexes only non-local ids, so a local bearer would page as 0/1.
  const positions = useMemo(() => {
    const bearers = visibleMessages.filter(
      (m) => m.role === "user" && !m.id.startsWith(LOCAL_PREFIX),
    );
    return forkPositions(allMessages, messages, bearers, (id) => id.startsWith(LOCAL_PREFIX));
  }, [messages, visibleMessages, allMessages]);
  const activeMessage = visibleMessages.at(-1);
  const transcriptAnnouncement = useTranscriptAnnouncement(messages);
  const pendingTailTool = busy ? streamTailTool(messages) : null;
  return (
    <>
      <span className="sr-only" role="status" aria-live="polite">
        <span key={transcriptAnnouncement.sequence}>{transcriptAnnouncement.text}</span>
      </span>
      {visibleMessages.map((m) => {
        const turnStatus = m.parts.find(isTurnStatusPart);
        const turnId = turnStatus?.state?.input?.turnId;
        // A session owns one turn slot, so one recovery disables every status
        // card until its durable admission resolves.
        const recoveryDisabled = turnStatus ? busy || recoveringTurnId !== null : false;
        return (
          <Message
            key={m.id}
            message={m}
            forkCount={positions.get(m.id)?.count}
            forkIndex={positions.get(m.id)?.index}
            forkPrevId={positions.get(m.id)?.prevId}
            forkNextId={positions.get(m.id)?.nextId}
            forkDisabled={!canFork}
            branchDisabled={busy}
            onFork={onFork}
            onSelectFork={onSelectFork}
            activePermissionId={activePermissionId}
            pendingTailToolId={pendingTailTool?.messageId === m.id ? pendingTailTool.toolId : null}
            onOpenFile={onOpenFile}
            onOpenRun={onOpenRun}
            onOpenSpawnedSession={onOpenSpawnedSession}
            runExperimentName={runExperimentName}
            onOpenExperiment={onOpenExperiment}
            experimentName={experimentName}
            onRespond={onRespond}
            onOpenPlan={onOpenPlan}
            onOpenSubagent={onOpenSubagent}
            busy={recoveryDisabled}
            recoveringTurnId={turnId === recoveringTurnId ? recoveringTurnId : null}
            onRecover={onRecover}
            skills={skills}
            predictTextTail={busy && m === activeMessage && m.role === "assistant"}
         />
        );
      })}
    </>
  );
});

// --- session rail ------------------------------------------------------------

type SessionFilter = "active" | "archived" | "all";

/** Whether the rail's current filter shows a session in this archived state. */
const matchesFilter = (filter: SessionFilter, archived: boolean) =>
  filter === "all" ? true : filter === "archived" ? archived : !archived;

/** Menu label + rail section heading per filter — "Recents" for the default view. */
const SESSION_FILTERS: { id: SessionFilter; label: () => string; railLabel: () => string }[] = [
  { id: "active", label: m.chat_panel_active, railLabel: m.chat_recents },
  { id: "archived", label: m.chat_panel_archived, railLabel: m.chat_panel_archived },
  { id: "all", label: m.chat_panel_all, railLabel: m.chat_all_sessions },
];

/** Filter control beside the "Recents" label: Active (default) / Archived / All. */
function SessionFilterMenu({
  value,
  onChange,
}: {
  value: SessionFilter;
  onChange: (next: SessionFilter) => void;
}) {
  const { open, setOpen, ref } = usePopover();
  return (
    <div className="rail-filter relative inline-flex" ref={ref}>
      <IconButton size="small"
        className="rail-filter-btn"
        active={value !== "active"}
        title={m.chat_panel_filter_sessions()}
        aria-label={m.chat_panel_filter_sessions()}
        onClick={() => setOpen((v) => !v)}
      >
        <SlidersHorizontal size={13} />
      </IconButton>
      {open && (
        <div className="option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down align-right">
          {SESSION_FILTERS.map((f) => (
            <MenuItem
              key={f.id}

              onClick={() => {
                onChange(f.id);
                setOpen(false);
              }}
            >
              <span>{f.label()}</span>
              {value === f.id && <Check size={13} />}
            </MenuItem>
          ))}
        </div>
      )}
    </div>
  );
}

/** Per-character stagger, and the ceiling on the whole run — a long title
 * shouldn't take a second and a half to finish arriving. */
const TITLE_CHAR_STAGGER_MS = 14;
const TITLE_STAGGER_CAP_MS = 500;
/** How long the reveal flag stays set: the capped stagger plus one character's
 * 240ms animation, plus slack. After this the title renders as plain text. */
const TITLE_REVEAL_CLEAR_MS = 1200;

/** A session title that materializes character by character when a
 * harness-generated one replaces the first-line placeholder. `animate` is false
 * everywhere else (initial load, renames, re-renders), and then this renders the
 * bare string — the animated form is deliberately the exception.
 *
 * The characters are `aria-hidden` and the whole title rides an `aria-label`:
 * a screen reader must hear one title, not forty single-letter spans. */
function TitleReveal({ title, animate }: { title: string; animate: boolean }) {
  if (!animate) return <>{title}</>;
  return (
    <span className="title-reveal" aria-label={title}>
      {Array.from(title).map((ch, i) =>
        // Spaces stay plain inline boxes: the animated characters must be
        // inline-block (transform doesn't apply to inline boxes), but an
        // inline-block space collapses to zero width and eats the word gap.
        ch === " " ? (
          <span key={i} aria-hidden>
            {ch}
          </span>
        ) : (
          <span
            key={i}
            aria-hidden
            className="title-reveal-char inline-block animate-[title-char-in_240ms_ease-out_both] [@media((prefers-reduced-motion:_reduce))]:animate-none"
            style={{
              animationDelay: `${Math.min(i * TITLE_CHAR_STAGGER_MS, TITLE_STAGGER_CAP_MS)}ms`,
            }}
          >
            {ch}
          </span>
        ),
      )}
    </span>
  );
}

/** One Recents row. Hover swaps the timestamp for a three-dot menu with
 * Rename, Archive/Unarchive, and Delete (Claude-desktop style). Rename turns
 * the title into an inline input. */
function SessionRow({
  session,
  active,
  unread,
  busy,
  waiting,
  revealTitle,
  onOpen,
  onRename,
  onSetArchived,
  onDelete,
}: {
  session: ChatSession;
  active: boolean;
  unread: boolean;
  busy: boolean;
  /** Turn held on an unanswered card: steady dot, not the working pulse. */
  waiting: boolean;
  /** Nonce set while this row's freshly auto-generated title should play its
   * reveal; it doubles as the remount key so a second retitle replays it.
   * Undefined the rest of the time (static title). */
  revealTitle: number | undefined;
  onOpen: () => void;
  onRename: (title: string) => void;
  onSetArchived: (archived: boolean) => void;
  onDelete: () => void;
}) {
  const { open, setOpen, ref } = usePopover();
  const title = session.title?.trim() || "Untitled";
  const [editing, setEditing] = useState(false);
  // Seeded by startEditing() before the input mounts; "" is just a placeholder.
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  function startEditing() {
    setDraft(session.title?.trim() || "");
    setEditing(true);
  }
  function commit() {
    const next = draft.trim();
    setEditing(false);
    // Only persist a real change; an empty title would be rejected server-side.
    if (next && next !== (session.title?.trim() || "")) onRename(next);
  }

  // Focus + select the input once the row enters edit mode.
  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  // Not a <button>: the kebab is a real button and can't nest inside one.
  return (
    <div
      ref={ref}
      role="button"
      tabIndex={0}
      className={`session-row relative flex items-center gap-2 w-full text-start py-[7px] px-2.5 rounded-md text-sm text-text cursor-pointer select-none [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium [&_.session-dot]:w-3.5 [&_.session-dot]:inline-flex [&_.session-dot]:items-center [&_.session-dot]:justify-center [&_.session-dot]:shrink-0 [&_.session-title]:flex-1 [&_.session-title]:min-w-0 [&_.session-title]:overflow-hidden [&_.session-title]:text-ellipsis [&_.session-title]:whitespace-nowrap [&.unread_.session-title]:font-semibold [&_.session-time]:text-xs [&_.session-time]:text-muted [&_.session-time]:shrink-0 [&_.session-menu-btn]:hidden [&_.session-menu-btn]:items-center [&_.session-menu-btn]:justify-center [&_.session-menu-btn]:w-4 [&_.session-menu-btn]:h-4 [&_.session-menu-btn]:-my-0.5 [&_.session-menu-btn]:mx-0 [&_.session-menu-btn]:rounded-sm [&_.session-menu-btn]:text-muted [&_.session-menu-btn]:shrink-0 [&_.session-menu-btn:hover]:text-text [&_.session-menu-btn:hover]:bg-panel [&:hover_.session-menu-btn]:inline-flex [&:focus-within_.session-menu-btn]:inline-flex [&.menu-open_.session-menu-btn]:inline-flex [&:hover_.session-time]:hidden [&:focus-within_.session-time]:hidden [&.menu-open_.session-time]:hidden [&_.busy-dot]:w-[7px] [&_.busy-dot]:h-[7px] [&_.busy-dot]:rounded-full [&_.busy-dot]:bg-primary [&_.busy-dot]:animate-[or-pulse_1.2s_infinite] [&_.busy-dot]:shrink-0 [&_.unread-dot]:w-[7px] [&_.unread-dot]:h-[7px] [&_.unread-dot]:rounded-full [&_.unread-dot]:bg-primary [&_.unread-dot]:shrink-0 [&_.busy-dot.waiting]:animate-none [&_.session-title-input]:flex-1 [&_.session-title-input]:min-w-0 [&_.session-title-input]:py-px [&_.session-title-input]:px-[5px] [&_.session-title-input]:-my-0.5 [&_.session-title-input]:mx-0 [&_.session-title-input]:[font:inherit] [&_.session-title-input]:text-text [&_.session-title-input]:bg-background [&_.session-title-input]:border [&_.session-title-input]:border-primary [&_.session-title-input]:rounded-sm [&_.session-title-input]:outline-none [&.editing]:bg-surface [&.editing]:cursor-default [&.editing_.session-menu-btn]:hidden [&.editing_.session-time]:hidden ${active ? "active" : ""}  ${unread ? "unread" : ""}  ${open ? "menu-open" : ""}  ${
        editing ? "editing" : ""
      }`}
      title={`${HARNESS_LABELS[session.harness]}${session.model ? ` · ${session.model}` : ""}${
        session.parentSessionId ? m.chat_spawned_by_agent() : ""
      }`}
      onClick={() => {
        // While editing, a body click is a no-op; blur/Enter/Esc drive it.
        if (editing) return;
        // With the menu open, a body click just dismisses it — switching
        // sessions underneath an open menu would leave it orphaned.
        if (open) setOpen(false);
        else onOpen();
      }}
      onKeyDown={(e) => {
        // Only keys aimed at the row itself: the kebab, menu items, and the
        // rename input are descendants, and preventDefault here would cancel
        // their activation.
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          // Mirror the click branch: dismiss an open menu instead of
          // navigating underneath it.
          if (open) setOpen(false);
          else onOpen();
        }
      }}
    >
      <span className="session-dot">
        {busy ? (
          <span className={`busy-dot ${waiting ? "waiting" : ""}`} />
        ) : (
          unread && <span className="unread-dot" />
        )}
      </span>
      {session.parentSessionId && !editing && (
        <Users className="text-muted shrink-0" size={12} aria-hidden />
      )}
      {editing ? (
        <input
          ref={inputRef}
          className="session-title-input"
          aria-label={m.chat_panel_session_title()}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onClick={(e) => e.stopPropagation()}
          onBlur={commit}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              setEditing(false);
            }
          }}
       />
      ) : (
        <span className="session-title">
          <TitleReveal
            key={revealTitle ?? "static"}
            title={title}
            animate={revealTitle !== undefined}
         />
        </span>
      )}
      <span className="session-time">{relTime(session.updatedAt)}</span>
      <button
        className="session-menu-btn"
        title={m.chat_panel_session_options()}
        aria-label={m.chat_panel_session_options()}
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        <MoreHorizontal size={14} />
      </button>
      {open && (
        <div className="option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down session-menu">
          <MenuItem
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              startEditing();
            }}
          >
            <span>{m.chat_panel_rename()}</span>
          </MenuItem>
          <MenuItem
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onSetArchived(!session.archived);
            }}
          >
            <span>{session.archived ? m.chat_unarchive() : m.chat_archive()}</span>
          </MenuItem>
          <MenuItem danger
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onDelete();
            }}
          >
            <span>{m.chat_panel_delete()}</span>
          </MenuItem>
        </div>
      )}
    </div>
  );
}

// --- panel -------------------------------------------------------------------

// The four starter prompts progress understand → gap → baseline → experiment.
const STARTER_ICONS = [BookOpen, Search, SquareTerminal, FlaskConical];
// One outline colour per step so the four boxes read as distinct choices.
const STARTER_TONES = [
  { box: "border-accent-blue/45", icon: "text-accent-blue" },
  { box: "border-accent-green/45", icon: "text-accent-green" },
  { box: "border-accent-amber/45", icon: "text-accent-amber" },
  { box: "border-primary/45", icon: "text-primary" },
];
const STARTER_GRID_CLASS =
  "mt-7 grid w-full max-w-readable grid-cols-1 gap-3 sm:grid-cols-2";

function RemoteHostDialog({
  onClose,
  onConfigureSsh,
}: {
  onClose: () => void;
  onConfigureSsh: () => void;
}) {
  const [hosts, setHosts] = useState<SshHost[] | null>(null);
  const [sessions, setSessions] = useState<RemoteSessionInfo[]>([]);
  const [query, setQuery] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [openingHost, setOpeningHost] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    Promise.all([getSshHosts(), listRemoteSessions()])
      .then(([nextHosts, nextSessions]) => {
        setHosts(nextHosts);
        setSessions(nextSessions);
      })
      .catch((error) => setLoadError(error instanceof Error ? error.message : String(error)));
  }, []);

  useDialogFocus(dialogRef, onClose);

  async function openRemote(host: string) {
    const remoteWindow = window.open("/remote-launch", "_blank");
    if (!remoteWindow) {
      showAlert(m.remote_popup_blocked(), "error");
      return;
    }
    setOpeningHost(host);
    try {
      const session = await createRemoteSession(host, {
        theme: localStorage.getItem("orx:theme"),
        locale: getLocale(),
      });
      remoteWindow.location.replace(session.gatewayUrl);
      onClose();
    } catch (error) {
      remoteWindow.close();
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setOpeningHost(null);
    }
  }

  const filteredHosts = hosts?.filter((host) =>
    host.host.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  const sessionByHost = new Map(sessions.map((session) => [session.host, session]));

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="relative flex h-[min(42rem,calc(100vh-2.5rem))] w-160 max-w-full flex-col overflow-hidden rounded-xl border border-border bg-background shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="remote-host-dialog-title"
        tabIndex={-1}
      >
        <IconButton className="absolute end-3.5 top-3.5" aria-label={m.remote_dialog_close()} onClick={onClose}>
          <X size={16} />
        </IconButton>
        <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
          <h2 id="remote-host-dialog-title" className="m-0 text-xl font-medium">{m.remote_dialog_title()}</h2>
          <p className="mt-2 mb-0 text-sm leading-normal text-subtext">{m.remote_dialog_description()}</p>
          <Input
            data-initial-focus
            className="mt-4"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={m.remote_search_hosts()}
            aria-label={m.remote_search_hosts()}
          />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto border-t border-border-variant p-2">
          {loadError ? (
            <p className="m-3 text-sm text-accent-red">{loadError}</p>
          ) : hosts === null ? (
            <div className="flex items-center gap-2 p-3 text-sm text-subtext"><Spinner /> {m.settings_page_reading_ssh_config()}</div>
          ) : filteredHosts?.length === 0 ? (
            <p className="m-3 text-sm text-subtext">{m.remote_no_matching_hosts()}</p>
          ) : (
            filteredHosts?.map((host) => {
              const session = sessionByHost.get(host.host);
              return (
                <Button
                  key={host.host}
                  variant="ghost"
                  className="w-full justify-start text-base font-normal"
                  disabled={openingHost === host.host}
                  onClick={() => void openRemote(host.host)}
                >
                  <span className="min-w-0 flex-1 truncate text-start">{host.host}</span>
                  {openingHost === host.host ? (
                    <Spinner />
                  ) : session ? (
                    <span className="text-sm text-subtext">{m.remote_open()}</span>
                  ) : null}
                </Button>
              );
            })
          )}
        </div>
        <div className="shrink-0 border-t border-border-variant p-2">
          <Button variant="ghost" className="w-full justify-start text-base font-normal" onClick={onConfigureSsh}>
            <SlidersHorizontal size={15} />
            {m.ssh_configure_hosts()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export function ChatPanel({
  projectId,
  projectName,
  railHeader,
  railOpen,
  onShowRail,
  mainView,
  onSelectMainView,
  experimentsActive,
  filesActive,
  artifactsActive,
  onOpenExperiments,
  onOpenArtifacts,
  onOpenFile,
  onOpenRun,
  runExperimentName,
  onOpenExperiment,
  experimentName,
  onOpenPlan,
  onOpenSubagent,
  onOpenWorktree,
  runtime,
  onOpenDemoWelcome,
  composerPrefill = null,
  onActiveSessionChange,
  preferredAgent,
  onPreferredAgentChange,
  children,
}: {
  projectId: string;
  projectName: string;
  /** Back-to-projects + project name block rendered at the top of the rail. */
  railHeader?: React.ReactNode;
  /** Whether the agents rail is showing (collapsed via its own header icon). */
  railOpen: boolean;
  /** Reopen the rail (from the chat header's sidebar icon). */
  onShowRail: () => void;
  /** Settings sections replace chat; Artifacts remains a right-panel tool. */
  mainView: "chat" | "skills" | SettingsTab;
  onSelectMainView: (view: "chat" | "skills" | SettingsTab) => void;
  experimentsActive: boolean;
  filesActive: boolean;
  artifactsActive: boolean;
  onOpenExperiments: () => void;
  onOpenArtifacts: () => void;
  /** Open a project file in the right pane (chat tool rows are clickable).
   * `sessionId` is the chat session the click came from, so relative paths
   * can resolve against that session's worktree. */
  onOpenFile?: (
    path: string,
    sessionId: string | undefined,
    line: number | undefined,
    exp: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  /** Open a run's logs in the right pane (agent-emitted `<run>` evidence chips).
   * Run ids are globally unique, so no session context is needed. */
  onOpenRun?: OpenTranscriptTarget;
  /** Resolve a run to the experiment name shown on tool activity links. */
  runExperimentName?: (runId: string) => string;
  /** Open an experiment overview, where its notes are displayed. */
  onOpenExperiment?: OpenTranscriptTarget;
  /** Resolve an experiment id to the name shown on tool activity links. */
  experimentName?: (experimentId: string) => string;
  /** Open a plan's markdown as a right-pane tab (plan strip / plan cards). */
  onOpenPlan?: (
    plan: string,
    sessionId: string,
    promptId: string,
    intent: TabOpenIntent,
  ) => void;
  /** Open a sub-agent's transcript as a right-pane tab (spawn-row "view").
   * `sessionId` is the chat session; `spawnPartId` locates the spawn part. */
  onOpenSubagent?: (
    sessionId: string,
    spawnPartId: string,
    label: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  /** Open the pinned Files home for the active session. */
  onOpenWorktree: () => void;
  runtime: RuntimeInfo;
  /** Reopen the demo welcome modal from the chat header. */
  onOpenDemoWelcome?: () => void;
  composerPrefill?: string | null;
  /** The open chat session, surfaced so the shell can scope panes to it. */
  onActiveSessionChange?: (sessionId: string | null) => void;
  /** Database-backed selection used to seed new chat sessions. */
  preferredAgent: ModelSelection | null;
  onPreferredAgentChange: (selection: ModelSelection) => Promise<void>;
  /** Middle-pane content when a settings section is active. */
  children?: React.ReactNode;
}) {
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [remoteDialogOpen, setRemoteDialogOpen] = useState(false);
  const [sshConfigOpen, setSshConfigOpen] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [unreadSessionIds, setUnreadSessionIds] = useState<ReadonlySet<string>>(new Set());
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>("active");
  const [draft, setDraft] = useState("");
  const [annotations, setAnnotations] = useState<ComposerAnnotation[]>([]);
  const annotationId = useRef(0);
  const composerScopeRef = useRef({ projectId, activeId });
  composerScopeRef.current = { projectId, activeId };
  // Pasted/dropped/uploaded attachments waiting in the composer, as data URLs.
  const [attachments, setAttachments] = useState<
    { dataUrl: string; mediaType: string; name?: string; size: number }[]
  >([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const settingsMutationTail = useRef<Promise<void>>(Promise.resolve());
  const settingsMutationSeq = useRef(0);
  const planMutationSeq = useRef(0);
  const [planModeOverride, setPlanModeOverride] = useState<boolean | null>(null);
  const planModeOverrideRef = useRef<boolean | null>(null);
  const queuedPlanOverrideSeen = useRef(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [state, dispatch] = useReducer(reducer, {
    messagesBySession: {},
    busySessions: new Set<string>(),
    queuedBySession: {},
    activeLeafBySession: {},
  });
  const [harnesses, setHarnesses] = useState<Harness[]>([]);
  const [selection, setSelection] = useState<ModelSelection | null>(preferredAgent);
  useEffect(() => setSelection(preferredAgent), [preferredAgent]);
  // Unsent composer tweaks (model/mode/reasoning) for the *open* session — the
  // session's harness is fixed, so these override only its mutable settings and
  // are applied (and persisted server-side) on the next send. Cleared when the
  // active session changes. Distinct from `selection`, which is the sticky
  // global preference that seeds *new* sessions.
  const [sessionOverride, setSessionOverride] = useState<Partial<ModelSelection>>({});
  const [recoveryOverrides, setRecoveryOverrides] = useState<
    Partial<ModelSelection> & { planMode?: boolean }
  >({});
  const [recoveringTurnId, setRecoveringTurnId] = useState<string | null>(null);
  const recoveringTurnRef = useRef(false);
  const activeLeafRef = useRef<string | null>(null);
  const [retryingQueuedId, setRetryingQueuedId] = useState<string | null>(null);
  const pendingClientTurn = useRef<{ signature: string; id: string } | null>(null);
  // Sessions whose title was just replaced by a harness-generated one, mapped
  // to a nonce that bumps per reveal so a second retitle remounts the spans and
  // replays the animation instead of sitting on a finished one.
  const [titleReveals, setTitleReveals] = useState<Map<string, number>>(new Map());
  // Last title seen per session id. The SSE subscription is keyed on projectId
  // alone, so its closure can't read `sessions`; this ref is what tells an
  // incoming title from the one already on screen.
  const seenTitles = useRef(new Map<string, string | null>());
  const loadedSessions = useRef(new Set<string>());
  // Tombstones: a turn finishing in the same instant as a delete can emit its
  // final chat.session upsert *after* chat.session.deleted; ignoring upserts
  // for known-deleted ids keeps the ghost row from coming back.
  const deletedIds = useRef(new Set<string>());
  // Bumped on every chat.message dispatch — the reconnect repair uses it to
  // detect a live flush racing its transcript refetch.
  const msgGen = useRef(0);
  // Render-fresh mirror of `sessions` for callbacks memoized on projectId
  // alone (syncSessionList snapshots it before fetching).
  const sessionsRef = useRef<ChatSession[]>([]);
  const threadRef = useRef<HTMLDivElement>(null);
  const threadInnerRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const [transcriptAtBottom, setTranscriptAtBottom] = useState(true);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const dataSources = usePopover();
  const addTranscriptSelection = useCallback((selection: Pick<SelectionAction, "text" | "range">) => {
    annotationId.current += 1;
    setAnnotations((current) => [
      ...current,
      { id: `annotation-${annotationId.current}`, ...selection },
    ]);
    composerRef.current?.focus();
  }, []);
  const transcriptSelection = useTranscriptSelection(threadInnerRef, addTranscriptSelection);
  useAnnotationHighlights(annotations);

  useEffect(() => {
    setAnnotations([]);
    transcriptSelection.dismiss();
  }, [activeId, projectId, transcriptSelection.dismiss]);

  // Slash-skills: menu state is derived from the draft — open while the token
  // under the caret is an unfinished `/command` (no whitespace yet) with
  // matches, wherever in the message it was typed.
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [skillIdx, setSkillIdx] = useState(0);
  const [skillMenuDismissed, setSkillMenuDismissed] = useState(false);
  const [composerCursor, setComposerCursor] = useState(0);
  // IME guard: mid-composition text can transiently look like a full command.
  const composingRef = useRef(false);

  // Refetch when navigating (esp. back to chat after a Skills-tab upload) so
  // freshly uploaded skills appear in the `/` menu without a reload.
  useEffect(() => {
    getSkills().then(setSkills).catch(() => {});
  }, [mainView]);
  // Only reachable while the menu is open, which needs a live slash context.
  function pickSkill(skill: SkillInfo) {
    if (!slashContext) return;
    if (skill.source === "command" && skill.name === "plan") {
      activatePlanCommand(draft, slashContext);
      return;
    }
    // The command replaces the `/query` token in place, so the chip lands where
    // it was typed and the rest of the message stays untouched.
    const next = insertSlashCommand(draft, slashContext, skill.name, 2);
    setDraft(next.text);
    window.requestAnimationFrame(() => {
      composerRef.current?.focus();
      composerRef.current?.setSelectionRange(next.cursor, next.cursor);
      setComposerCursor(next.cursor);
    });
  }

  /** Backspace just behind a chip deletes the whole command, the way the chip
   * it paints reads — a single object, not eight characters. */
  function deleteCommandBehindCaret(textarea: HTMLTextAreaElement): boolean {
    const cursor = textarea.selectionStart;
    if (composingRef.current || cursor !== textarea.selectionEnd) return false;
    const context = slashCommandContext(draft, cursor);
    if (!context || context.end !== cursor) return false;
    if (!knownCommand(context.query)) return false;
    const next = removeSlashCommand(draft, context);
    setDraft(next.text);
    setComposerCursor(next.cursor);
    window.requestAnimationFrame(() =>
      textarea.setSelectionRange(next.cursor, next.cursor),
    );
    return true;
  }

  /** Queue files (upload button, clipboard paste, or drag-drop) as composer
   * attachments — images and PDFs, which the harness reads off disk by path. */
  function addFiles(files: File[]) {
    // Per-file and total caps keep the base64-inflated (~33%) request body
    // under the backend's 64 MB limit — a single 30 MB file or a batch summing
    // to 40 MB both stay clear once encoded.
    const MAX_BYTES = 30 * 1024 * 1024;
    const TOTAL_BYTES = 40 * 1024 * 1024;
    setAttachError(null);
    let total = attachments.reduce((n, a) => n + a.size, 0);
    for (const file of files) {
      if (!/^(image\/(png|jpeg|gif|webp)|application\/pdf)$/.test(file.type)) continue;
      if (file.size > MAX_BYTES) {
        setAttachError(m.chat_attachment_too_large({ name: ltr(file.name) }));
        continue;
      }
      if (total + file.size > TOTAL_BYTES) {
        setAttachError(m.chat_attachments_total_too_large());
        continue;
      }
      total += file.size;
      const reader = new FileReader();
      reader.onload = () => {
        const dataUrl = reader.result as string;
        setAttachments((cur) => [
          ...cur,
          { dataUrl, mediaType: file.type, name: file.name, size: file.size },
        ]);
      };
      reader.readAsDataURL(file);
    }
  }

  function onComposerPaste(e: React.ClipboardEvent) {
    const files = Array.from(e.clipboardData.items)
      .filter(
        (item) =>
          item.kind === "file" &&
          (item.type.startsWith("image/") || item.type === "application/pdf"),
      )
      .map((item) => item.getAsFile())
      .filter((f): f is File => f !== null);
    if (files.length > 0) {
      e.preventDefault();
      addFiles(files);
    }
  }

  // The open session, if any (its harness is locked; its model/mode/reasoning
  // are what the composer should reflect and edit).
  const openSession = sessions.find((s) => s.id === activeId);

  // The selection the composer displays and edits:
  //  * with a session open — that session's stored settings, with any unsent
  //    picker tweaks layered on. The harness is the session's, not the global.
  //  * with no session — the sticky global preference (seeds a new session).
  const savedSelection = selection ?? defaultSelection(harnesses);
  const rawSelection: ModelSelection | null = openSession
    ? {
        harness: openSession.harness,
        model: sessionOverride.model ?? openSession.model,
        serviceTier:
          sessionOverride.serviceTier !== undefined
            ? sessionOverride.serviceTier
            : openSession.serviceTier,
        permissionMode: sessionOverride.permissionMode ?? openSession.permissionMode,
        reasoningLevel: sessionOverride.reasoningLevel ?? openSession.reasoningLevel,
      }
    : savedSelection
      ? { ...savedSelection, ...sessionOverride }
      : null;
  const activeHarness = rawSelection
    ? harnesses.find((h) => h.id === rawSelection.harness)
    : undefined;
  const opts = activeHarness?.options;
  const commands = useMemo(
    () => commandsForHarness(skills, opts?.planActivation),
    [skills, opts?.planActivation],
  );
  const slashContext = slashCommandContext(draft, composerCursor);
  const slashToken = slashContext?.query ?? null;
  // Commands now live in the draft as text, so the menu also has to stay shut
  // when the caret merely lands in or behind a name the user already finished —
  // unless a longer command still extends it.
  const completions =
    slashToken === null ? [] : commands.filter((command) => command.name.startsWith(slashToken));
  const typingCommand =
    slashToken !== null &&
    slashContext?.end === composerCursor &&
    completions.some((command) => command.name !== slashToken);
  const skillMatches =
    typingCommand && !skillMenuDismissed ? completions : [];
  const skillMenuOpen = skillMatches.length > 0;
  const activeSkillIdx = Math.min(skillIdx, Math.max(0, skillMatches.length - 1));
  useEffect(() => setSkillIdx(0), [slashToken]);
  // Reconcile the reasoning level against the *currently selected model* here
  // rather than only in the picker's `pick`. Two paths reach the composer with
  // a level nobody chose for this model: a session row stored by an older build
  // (which always wrote an explicit effort), and a stale saved preference.
  // Reconciling at the point the composer derives its state covers both, so the
  // displayed value and the value `send` transmits can never be one the model
  // rejects.
  // A saved model the harness no longer lists (a provider whose key was
  // rejected, a retired id) falls back to the harness's first model.
  const composerModel =
    rawSelection &&
    activeHarness &&
    activeHarness.models.length > 0 &&
    !activeHarness.models.some((model) => model.id === rawSelection.model)
      ? activeHarness.models[0].id
      : (rawSelection?.model ?? null);
  const composerSelection: ModelSelection | null = rawSelection && {
    ...rawSelection,
    model: composerModel,
    serviceTier: reconcileServiceTier(activeHarness, composerModel, rawSelection.serviceTier),
    reasoningLevel: reconcileReasoning(activeHarness, composerModel, rawSelection.reasoningLevel),
  };
  // Reasoning choices follow the *selected model*, not just the harness — an
  // OpenCode model with no `variants` hides the picker entirely, and Codex's
  // top tiers appear only on the models that accept them.
  const reasoning = reasoningFor(activeHarness, composerSelection?.model);

  // Editing the pickers: every change updates the sticky global preference —
  // the config a "New session" composer opens with is whatever the user chose
  // LAST, whether they chose it on an empty composer or inside a session. With
  // a session open the change additionally lands as that session's unsent
  // tweak (applied on the next send).
  //
  // The session override is *merged*, never replaced. It has to be: the pickers
  // build their `next` by spreading `composerSelection`, whose reasoning level
  // is a reconciled value rather than the session's stored one. Replacing would
  // let a change on one axis pin a reconciled value on another — picking a
  // permission mode would write a reasoning level the user never chose, and the
  // next send would persist it over their real setting.
  const selectModel = (next: Partial<ModelSelection>) => {
    if (!composerSelection) return;
    const merged = { ...composerSelection, ...next };
    const changed: Partial<ModelSelection> = {};
    if (next.model !== undefined && next.model !== composerSelection.model) changed.model = next.model;
    if (
      next.serviceTier !== undefined &&
      next.serviceTier !== composerSelection.serviceTier
    )
      changed.serviceTier = next.serviceTier;
    if (
      next.permissionMode !== undefined &&
      next.permissionMode !== composerSelection.permissionMode
    )
      changed.permissionMode = next.permissionMode;
    if (
      next.reasoningLevel !== undefined &&
      next.reasoningLevel !== composerSelection.reasoningLevel
    )
      changed.reasoningLevel = next.reasoningLevel;
    setRecoveryOverrides((current) => ({ ...current, ...changed }));
    setSelection(merged);
    void onPreferredAgentChange(merged).catch(() => {});
    if (openSession) {
      setSessionOverride((cur) => ({ ...cur, ...next }));
    } else if (next.harness && next.harness !== composerSelection.harness) {
      setSessionOverride({});
    }
  };
  const queueSessionMutation = useCallback(<T,>(mutation: () => Promise<T>): Promise<T> => {
    const result = settingsMutationTail.current.catch(() => {}).then(mutation);
    settingsMutationTail.current = result.then(
      () => {},
      () => {},
    );
    return result;
  }, []);
  const setPermissionMode = (id: string) => {
    // Plan is session-scoped. Claude exposes it in the permission dropdown,
    // but it must not become the saved default for future sessions.
    if (id === "plan" && activeHarness?.id === "claude-code") {
      setRecoveryOverrides((current) => ({ ...current, permissionMode: id }));
      setSessionOverride((current) => ({ ...current, permissionMode: id }));
    } else {
      setSessionOverride((current) => {
        const next = { ...current };
        delete next.permissionMode;
        return next;
      });
      selectModel({ permissionMode: id });
    }
    if (!openSession) return;
    const sessionId = openSession.id;
    const mutation = ++settingsMutationSeq.current;
    setSettingsError(null);
    void queueSessionMutation(() => setChatSessionPermissionMode(sessionId, id))
      .then((session) => {
        setSessions((current) =>
          current.map((candidate) => (candidate.id === session.id ? session : candidate)),
        );
        if (settingsMutationSeq.current === mutation) {
          setSessionOverride((current) => {
            const next = { ...current };
            delete next.permissionMode;
            return next;
          });
        }
      })
      .catch(() => {
        if (settingsMutationSeq.current !== mutation) return;
        setSessionOverride((current) => {
          const next = { ...current };
          delete next.permissionMode;
          return next;
        });
        setSettingsError(m.chat_update_permissions_failed());
      });
  };
  const setReasoningLevel = (id: string) => selectModel({ reasoningLevel: id });
  const planActive = composerSelection?.harness === "claude-code"
    ? composerSelection.permissionMode === "plan"
    : opts?.planActivation === "command"
      ? planModeOverride ?? openSession?.planMode ?? false
      : false;
  useEffect(() => {
    if (planModeOverride === null || openSession?.planMode !== planModeOverride) return;
    planModeOverrideRef.current = null;
    setPlanModeOverride(null);
  }, [openSession?.planMode, planModeOverride]);

  async function setIndependentPlanMode(planMode: boolean) {
    setRecoveryOverrides((current) => ({ ...current, planMode }));
    planModeOverrideRef.current = planMode;
    setPlanModeOverride(planMode);
    if (!openSession) return;
    const sessionId = openSession.id;
    const mutation = ++planMutationSeq.current;
    setSettingsError(null);
    try {
      const session = await queueSessionMutation(() => setChatSessionPlanMode(sessionId, planMode));
      setSessions((current) =>
        current.map((candidate) => (candidate.id === session.id ? session : candidate)),
      );
      if (planMutationSeq.current === mutation) {
        planModeOverrideRef.current = null;
        setPlanModeOverride(null);
        setSettingsError(null);
      }
    } catch (error) {
      if (planMutationSeq.current === mutation) {
        planModeOverrideRef.current = null;
        setPlanModeOverride(null);
      }
      throw error;
    }
  }

  async function exitPlanMode() {
    if (composerSelection?.harness === "claude-code") {
      setPermissionMode("auto");
      return;
    }
    if (!openSession) return;
    try {
      await setIndependentPlanMode(false);
    } catch {
      setSettingsError(m.chat_exit_plan_failed());
    }
  }

  async function togglePlanMode() {
    const nextPlanMode = !planActive;
    try {
      if (composerSelection?.harness === "claude-code") {
        setPermissionMode(nextPlanMode ? "plan" : "auto");
      } else if (opts?.planActivation === "command") {
        await setIndependentPlanMode(nextPlanMode);
      } else {
        throw new Error(m.chat_selected_harness_unavailable());
      }
    } catch {
      setSettingsError(m.chat_toggle_plan_failed());
    }
  }

  function activatePlanCommand(text: string, context: SlashCommandContext) {
    const next = removeSlashCommand(text, context);
    setDraft(next.text);
    setSkillMenuDismissed(true);
    void togglePlanMode();
    window.requestAnimationFrame(() => {
      composerRef.current?.focus();
      composerRef.current?.setSelectionRange(next.cursor, next.cursor);
      setComposerCursor(next.cursor);
    });
  }

  sessionsRef.current = sessions;

  /** Fetch the authoritative session list and adopt it wholesale: the rows
   * (honoring delete tombstones and keeping locally-newer contextUsage — same
   * merge as the chat.session handler), the seenTitles baseline (so the next
   * live event compares against what's on screen rather than animating a title
   * the user already had), and the busy set. A session we showed that the
   * authoritative list no longer has was deleted while SSE was down (its
   * chat.session.deleted frame is lost for good) — run the full forget
   * cleanup, or its cached transcript, busy flag, and active selection linger
   * as a ghost. Shared by the project-change load and the SSE-reconnect
   * repair. Resolves to the adopted list, null on fetch failure. */
  const syncSessionList = useCallback(async (): Promise<ChatSession[] | null> => {
    // Snapshot BEFORE the fetch: a session created while the request is in
    // flight is absent from the response but also absent here, so it can
    // never be mistaken for deleted (forgetSession tombstones — a false
    // positive would kill a live session for good).
    const before = sessionsRef.current.map((s) => s.id);
    try {
      const list = (await listChatSessions(projectId)).filter(
        (s) => !deletedIds.current.has(s.id),
      );
      const ids = new Set(list.map((s) => s.id));
      // Forget BEFORE seeding busy: forget drops the ghost's busy flag, so
      // the known-scoped seed below can't carry it forward as if the session
      // belonged to another project.
      for (const id of before) if (!ids.has(id)) forgetSession(id);
      // Same contextUsage-preservation rule as the chat.session handler (the
      // scope differs: this replaces the whole array, that merges one row).
      setSessions((cur) => {
        const prevUsage = new Map(cur.map((c) => [c.id, c.contextUsage]));
        return list.map((s) => ({
          ...s,
          contextUsage: s.contextUsage ?? prevUsage.get(s.id),
        }));
      });
      seenTitles.current = new Map(list.map((s) => [s.id, s.title]));
      dispatch({
        type: "seedBusy",
        sessions: list.filter((s) => s.busy).map((s) => s.id),
        known: list.map((s) => s.id),
      });
      return list;
    } catch {
      return null;
    }
  }, [projectId]);

  const reseedSession = useCallback(
    async (sessionId: string) => {
      const leafBefore = composerScopeRef.current.activeId === sessionId
        ? activeLeafRef.current
        : undefined;
      const [{ messages, queued, activeLeafId }] = await Promise.all([
        getChatMessages(sessionId),
        syncSessionList(),
      ]);
      const localLeafMoved = leafBefore !== undefined
        && composerScopeRef.current.activeId === sessionId
        && activeLeafRef.current !== leafBefore;
      dispatch({
        type: "seed",
        sessionId,
        messages,
        queued,
        activeLeafId: localLeafMoved ? activeLeafRef.current : activeLeafId,
      });
    },
    [syncSessionList, dispatch],
  );

  // Reset everything when the project changes.
  useEffect(() => {
    setSessions([]);
    // Clear the mirror NOW, not at the next render: syncSessionList below
    // snapshots it, and the old project's rows would all read as "deleted"
    // against the new project's list — tombstoning the entire old project.
    sessionsRef.current = [];
    setActiveId(null);
    const readDemoSessions = loadReadDemoSessions();
    setUnreadSessionIds(
      projectId === DEMO_PROJECT_ID
        ? new Set(
            [DEMO_FIGURE_SESSION_ID, DEMO_LITERATURE_SESSION_ID].filter(
              (sessionId) => !readDemoSessions.has(sessionId),
            ),
          )
        : new Set(),
    );
    setDraft("");
    setAttachments([]);
    dispatch({ type: "reset" });
    loadedSessions.current = new Set();
    setTitleReveals(new Map());
    seenTitles.current = new Map();
    void syncSessionList().then((list) => {
      // Prefer the newest non-archived session; archived ones stay hidden.
      if (list)
        setActiveId(
          (cur) =>
            cur ??
            (projectId === DEMO_PROJECT_ID
              ? list.find((session) => session.id === DEMO_MAIN_SESSION_ID)?.id
              : undefined) ??
            list.find((session) => !session.archived)?.id ??
            null,
        );
    });
  }, [projectId, syncSessionList]);

  // Load message history when a session becomes active.
  useEffect(() => {
    setRecoveryOverrides({});
    pendingClientTurn.current = null;
  }, [activeId]);

  useEffect(() => {
    if (!activeId || loadedSessions.current.has(activeId)) return;
    loadedSessions.current.add(activeId);
    getChatMessages(activeId)
      .then(({ messages, queued, activeLeafId }) =>
        dispatch({ type: "seed", sessionId: activeId, messages, queued, activeLeafId }),
      )
      .catch(() => {
        // Recover from a failed fetch to a usable state rather than a stuck
        // "Loading conversation…" spinner: seed an empty transcript (clears
        // historyLoading, falls through to the empty state) unless messages
        // already streamed in, and drop the loadedSessions guard so switching
        // back to this session refetches.
        dispatch({ type: "seed", sessionId: activeId, messages: [], onlyIfAbsent: true });
        loadedSessions.current.delete(activeId);
      });
  }, [activeId]);

  // Chat events from the shared /api/events stream.
  useEffect(() => {
    return onChatEvent((ev) => {
      switch (ev.type) {
        case "session": {
          if (ev.session.projectId !== projectId) return;
          if (deletedIds.current.has(ev.session.id)) return;
          // A generated title landing on a session already on screen is the
          // auto-title arriving — reveal it. A session we've never seen is
          // skipped on purpose: a list load or a newly created row must not
          // animate a title that was simply always there.
          const known = seenTitles.current.has(ev.session.id);
          const changed = seenTitles.current.get(ev.session.id) !== ev.session.title;
          seenTitles.current.set(ev.session.id, ev.session.title);
          if (known && changed && ev.session.titleSource === "generated") {
            setTitleReveals((cur) => {
              const next = new Map(cur);
              next.set(ev.session.id, (cur.get(ev.session.id) ?? 0) + 1);
              return next;
            });
            // Drop the flag once the run is over (longest stagger + one char
            // duration, plus slack) so later re-renders show a static title.
            window.setTimeout(() => {
              setTitleReveals((cur) => {
                if (!cur.has(ev.session.id)) return cur;
                const next = new Map(cur);
                next.delete(ev.session.id);
                return next;
              });
            }, TITLE_REVEAL_CLEAR_MS);
          }
          setSessions((cur) => {
            const i = cur.findIndex((s) => s.id === ev.session.id);
            if (i < 0) return [ev.session, ...cur];
            const next = cur.slice();
            // An interrupted turn aborts before the persist block, so its
            // follow-up chat.session can lack usage the client already showed
            // live. Usage is never legitimately cleared, so keep the local
            // value whenever the incoming session omits one.
            next[i] = { ...ev.session, contextUsage: ev.session.contextUsage ?? cur[i].contextUsage };
            return next;
          });
          break;
        }
        case "sessionDeleted":
          forgetSession(ev.sessionId);
          break;
        case "message":
          msgGen.current++;
          dispatch({ type: "upsertMessage", sessionId: ev.sessionId, message: ev.message });
          break;
        case "busy":
          dispatch({ type: "busy", sessionId: ev.sessionId, busy: ev.busy });
          break;
        case "queued":
          dispatch({ type: "setQueued", sessionId: ev.sessionId, items: ev.items });
          break;
        case "branch":
          dispatch({ type: "activeLeaf", sessionId: ev.sessionId, leafId: ev.activeLeafId });
          break;
        case "usage":
          setSessions((cur) =>
            cur.map((s) => (s.id === ev.sessionId ? { ...s, contextUsage: ev.usage } : s)),
          );
          break;
      }
    });
  }, [projectId]);

  // Repair after an SSE gap. Chat frames are edge-only — a dropped EventSource
  // mid-turn loses chat.message / chat.busy events for good, which strands the
  // UI (a spinner that never clears, or a reply that never appears until a
  // reload). On reconnect, refetch the authoritative state: the session list
  // (busy flags ride it) and the active transcript. The seed replaces the
  // transcript wholesale, so a live flush racing the fetch would be clobbered
  // — and if it was the turn's FINAL flush, never repaired; the msgGen check
  // refetches once when that race is detected. Separate subscription so it can
  // depend on activeId without re-running the main handler's effect.
  useEffect(() => {
    return onChatEvent((ev) => {
      if (ev.type !== "reconnected") return;
      void syncSessionList();
      if (!activeId || !loadedSessions.current.has(activeId)) return;
      // One retry is sufficient: flush persists to the store BEFORE it emits,
      // so a refetch issued after observing a raced event already reads that
      // event's content.
      const reseed = (allowRetry: boolean) => {
        const gen = msgGen.current;
        getChatMessages(activeId)
          .then(({ messages, queued, activeLeafId }) => {
            dispatch({ type: "seed", sessionId: activeId, messages, queued, activeLeafId });
            if (allowRetry && msgGen.current !== gen) reseed(false);
          })
          .catch(() => {});
      };
      reseed(true);
    });
  }, [activeId, syncSessionList]);

  // Every fork stays loaded; only the branch on screen drives the transcript,
  // the streaming tail, and the pending-permission lookup.
  const allMessages = activeId ? (state.messagesBySession[activeId] ?? NO_MESSAGES) : NO_MESSAGES;
  const activeLeafId = activeId ? (state.activeLeafBySession[activeId] ?? null) : null;
  activeLeafRef.current = activeLeafId;
  const messages = useMemo(() => activePath(allMessages, activeLeafId), [allMessages, activeLeafId]);
  const busy = activeId ? state.busySessions.has(activeId) : false;
  const canFork = !busy && !!activeHarness?.agentReady;
  const hasPendingTailTool = busy && streamTailTool(messages) != null;
  const hasStreamingText = busy && streamTailIsText(messages);
  // Messages the user parked behind the running turn (oldest first). Populated
  // by chat.queued events and the seed snapshot; each runs when its turn ends.
  const queued = activeId ? (state.queuedBySession[activeId] ?? []) : [];
  const hasRetryingQueue = queued.some((item) => item.dispatchState === "retrying");
  const firstBlockedQueueIndex = queued.findIndex((item) => item.dispatchState === "blocked");
  const nextQueueRetryAt = queued.reduce<number | null>((latest, item) => {
    if (item.dispatchState !== "retrying" || typeof item.nextRetryAt !== "number") return latest;
    return latest === null ? item.nextRetryAt : Math.min(latest, item.nextRetryAt);
  }, null);
  const [queueClock, setQueueClock] = useState(() => Date.now());
  useEffect(() => {
    if (!hasRetryingQueue || nextQueueRetryAt === null) return;
    setQueueClock(Date.now());
    if (nextQueueRetryAt <= Date.now()) return;
    const timer = window.setInterval(() => {
      const current = Date.now();
      setQueueClock(current);
      if (current >= nextQueueRetryAt) window.clearInterval(timer);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [hasRetryingQueue, nextQueueRetryAt]);
  useEffect(() => {
    const queuedMode = queued.reduce<boolean | undefined>(
      (mode, item) => item.planMode ?? mode,
      undefined,
    );
    if (queuedMode !== undefined) {
      queuedPlanOverrideSeen.current = true;
      planModeOverrideRef.current = queuedMode;
      setPlanModeOverride(queuedMode);
    } else if (queuedPlanOverrideSeen.current) {
      queuedPlanOverrideSeen.current = false;
      planModeOverrideRef.current = null;
      setPlanModeOverride(null);
    }
  }, [queued]);
  // A session whose transcript hasn't been seeded yet: its key is absent from
  // messagesBySession (vs. present-but-empty for a genuinely empty session).
  // Switching to an existing session leaves this true for the getChatMessages
  // fetch, so we show a spinner instead of flashing the empty state. A brand-new
  // session created via the composer never lands here — its optimisticUser seed
  // populates the key synchronously in the same handler.
  const historyLoading = !!activeId && !(activeId in state.messagesBySession);
  // A busy turn blocked on an unanswered HELD card (nativeId — a bridge or
  // inline mid-turn request) is waiting on the user, not the model. Drives
  // the status line and the rail dot (the composer button is keyed on
  // `pendingQuestion` instead — what send() can actually service). End-turn
  // cards (no nativeId) never coexist with a busy turn of their own, so
  // keying on nativeId avoids false positives from stale cards. (Sessions
  // whose transcripts aren't loaded fall back to plain busy.) Memoized so the
  // messages × parts scan stays off the per-keystroke render path.
  const waitingSessions = useMemo(() => {
    const waiting = new Set<string>();
    for (const id of state.busySessions) {
      if (
        (state.messagesBySession[id] ?? []).some((m) =>
          m.parts.some(
            (p) => p.type === "prompt" && p.prompt && !p.prompt.resolved && p.prompt.nativeId,
          ),
        )
      )
        waiting.add(id);
    }
    return waiting;
  }, [state.busySessions, state.messagesBySession]);
  const awaitingInput = activeId ? waitingSessions.has(activeId) : false;
  const activeSession = openSession;
  // Nonce while the open session's title is mid-reveal; undefined = static.
  const activeTitleReveal = activeSession ? titleReveals.get(activeSession.id) : undefined;

  // The newest unresolved plan prompt, if any — it drives the docked strip
  // above the composer. Resolution re-emits the message over SSE, so this
  // recomputes to null and the strip disappears on its own.
  const pendingPlan = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      for (const part of messages[i].parts) {
        if (part.type === "prompt" && part.prompt?.kind === "plan" && !part.prompt.resolved) {
          return {
            promptId: part.id,
            plan: part.prompt.plan ?? "",
            synthesized: !!part.prompt.synthesized,
          };
        }
      }
    }
    return null;
  }, [messages]);

  // The newest ANSWERABLE unresolved question card's part id: typed composer
  // text answers IT as a custom answer, instead of racing the held turn with
  // a new message (which the busy guard would reject/drop). Plan cards have
  // their own inline revise textarea (PlanStrip) and don't route through
  // here. Claude + Codex sessions: both accept a note-only reply (codex's
  // user_input_reply takes the note as the surfaced question's freeform
  // answer). Opencode is excluded — it rejects note-only replies (see
  // reply_inline), so its options stay the interface. A held (nativeId) card
  // is answerable only while its turn is alive — a zombie left by a process
  // restart must not capture the composer (its own buttons error and the
  // backend collapses it on the first attempt).
  const pendingQuestion = useMemo(() => {
    const harness = activeSession?.harness;
    if (!activeId || (harness !== "claude-code" && harness !== "codex")) return null;
    for (let i = messages.length - 1; i >= 0; i--) {
      for (const part of messages[i].parts) {
        if (part.type !== "prompt" || !part.prompt || part.prompt.resolved) continue;
        if (part.prompt.kind !== "question") continue;
        if (part.prompt.nativeId && !state.busySessions.has(activeId)) return null;
        return part.id;
      }
    }
    return null;
  }, [messages, activeSession?.harness, activeId, state.busySessions]);

  // A pending question card owns the composer's text as a plain answer — no
  // command in it is ever expanded, so none of it is chipped either.
  const knownCommand = (name: string) =>
    !pendingQuestion && commands.some((command) => command.name === name);
  // A submitted plan revision, until its replacement card arrives: hides the
  // outgoing card's strip so it never sits there looking actionable while
  // the model rewrites the plan (the transcript's Working… spinner is the
  // feedback). Cleared when the session's turn ends or a DIFFERENT plan card
  // shows up in the same session — pendingPlan derives from the ACTIVE
  // session, so the replaced check must not fire on a session switch.
  const [revising, setRevising] = useState<{ sessionId: string; promptId: string } | null>(null);
  const revisingPlan = revising && revising.sessionId === activeId ? revising : null;
  useEffect(() => {
    if (!revising) return;
    const stillBusy = state.busySessions.has(revising.sessionId);
    const replaced =
      revising.sessionId === activeId && pendingPlan && pendingPlan.promptId !== revising.promptId;
    if (!stillBusy || replaced) setRevising(null);
  }, [revising, pendingPlan, state.busySessions, activeId]);

  const pendingPermission = useMemo(() => firstPendingPermission(messages), [messages]);
  // Enter hands the message to the running turn. Attachments still park (only
  // a full turn builds their on-disk preamble), and a pending card owns typed
  // text — there the card is the affordance, not a steer.
  const steering =
    busy &&
    !!activeHarness?.supportsSteering &&
    !!activeHarness?.agentReady &&
    !pendingPlan &&
    !pendingQuestion &&
    !pendingPermission &&
    attachments.length === 0 &&
    annotations.length === 0;

  // Plan opens are stamped with the session like file opens are. Memoized
  // (along with openFileInSession and respond below) so the memoized Message
  // rows don't all re-render on every streaming tick.
  const openPlan = useMemo(
    () =>
      onOpenPlan && activeId
        ? (plan: string, promptId: string, intent: TabOpenIntent) =>
            onOpenPlan(plan, activeId, promptId, intent)
        : undefined,
    [onOpenPlan, activeId],
  );

  const openSubagent = useMemo(
    () =>
      onOpenSubagent && activeId
        ? (spawnPartId: string, label: string | undefined, intent: TabOpenIntent) =>
            onOpenSubagent(activeId, spawnPartId, label, intent)
        : undefined,
    [onOpenSubagent, activeId],
  );

  // File opens resolve against the active session's worktree — the agent runs
  // there, so that's where its paths point.
  const openFileInSession = useMemo(
    () =>
      onOpenFile &&
      ((
        path: string,
        line: number | undefined,
        exp: string | undefined,
        ref: string | undefined,
        intent: TabOpenIntent,
      ) => onOpenFile(path, activeId ?? undefined, line, exp, ref, intent)),
    [onOpenFile, activeId],
  );

  // Drop any unsent composer tweak when switching sessions, so it never bleeds
  // from one session's pickers onto another's.
  useEffect(() => {
    settingsMutationSeq.current += 1;
    planMutationSeq.current += 1;
    const queuedMode = (activeId ? state.queuedBySession[activeId] ?? [] : []).reduce<
      boolean | undefined
    >((mode, item) => item.planMode ?? mode, undefined);
    queuedPlanOverrideSeen.current = queuedMode !== undefined;
    planModeOverrideRef.current = queuedMode ?? null;
    setPlanModeOverride(queuedMode ?? null);
    setSessionOverride({});
    setSettingsError(null);
  }, [activeId]);

  // Surface the open session to the shell (Agent-scoped panes key off it).
  useEffect(() => {
    onActiveSessionChange?.(activeId);
  }, [activeId, onActiveSessionChange]);

  // Opening a session or returning from settings starts pinned at the latest messages.
  const threadMounted = mainView === "chat" && (messages.length > 0 || busy);

  // Keyed by project+harness so a switch never shows another project's prompts.
  const starterHarness = composerSelection?.harness ?? null;
  const starterModel = composerSelection?.model ?? null;
  const [starter, setStarter] = useState<{
    key: string;
    prompts: StarterPrompt[] | null;
  } | null>(null);
  const starterKey = `${projectId}\0${starterHarness ?? ""}\0${starterModel ?? ""}`;
  const starterVisible = mainView === "chat" && !threadMounted && !historyLoading;
  useEffect(() => {
    if (!starterVisible || !starterHarness) return;
    let current = true;
    getProjectStarterPrompts(projectId, starterHarness, starterModel, getLocale())
      .then((result) => {
        if (current) setStarter({ key: starterKey, prompts: result.prompts });
      })
      .catch(() => {
        if (current) setStarter({ key: starterKey, prompts: null });
      });
    return () => {
      current = false;
    };
  }, [projectId, starterHarness, starterModel, starterKey, starterVisible]);
  const starterPrompts = starter?.key === starterKey ? starter.prompts : null;
  const starterLoading = starterHarness !== null && starter?.key !== starterKey;
  const applyStarterPrompt = (prompt: string) => {
    setDraft(prompt);
    setSkillMenuDismissed(false);
    window.requestAnimationFrame(() => {
      const el = composerRef.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(prompt.length, prompt.length);
      setComposerCursor(prompt.length);
    });
  };
  // Seeds the draft while a prefill is offered without taking focus, which may
  // belong to the demo welcome dialog; clearing the prefill later leaves the draft.
  useEffect(() => {
    if (!composerPrefill) return;
    setDraft(composerPrefill);
    setSkillMenuDismissed(false);
    setComposerCursor(composerPrefill.length);
  }, [composerPrefill]);
  const updateTranscriptBottom = useCallback((el: HTMLDivElement) => {
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    stickToBottom.current = atBottom;
    setTranscriptAtBottom(atBottom);
  }, []);
  const pinTranscriptToBottom = useCallback(() => {
    stickToBottom.current = true;
    setTranscriptAtBottom(true);
    const el = threadRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);
  useLayoutEffect(() => {
    pinTranscriptToBottom();
  }, [activeId, threadMounted, pinTranscriptToBottom]);

  // Autoscroll while pinned. Layout effect, so history seeds and streamed
  // messages land already scrolled (no flash of the top of the thread).
  useLayoutEffect(() => {
    if (stickToBottom.current) pinTranscriptToBottom();
  }, [messages, busy, pinTranscriptToBottom]);

  // Re-pin when the thread resizes without a message change — images loading,
  // tool rows expanding, the pane resizing.
  useEffect(() => {
    const el = threadRef.current;
    const inner = threadInnerRef.current;
    if (!el || !inner) return;
    const ro = new ResizeObserver(() => {
      if (stickToBottom.current) {
        el.scrollTop = el.scrollHeight;
        return;
      }
      updateTranscriptBottom(el);
    });
    ro.observe(inner);
    ro.observe(el);
    return () => ro.disconnect();
  }, [threadMounted, updateTranscriptBottom]);

  const scrollToTranscriptBottom = useCallback((event: ReactMouseEvent<HTMLButtonElement>) => {
    event.currentTarget.blur();
    pinTranscriptToBottom();
  }, [pinTranscriptToBottom]);

  /** `queue` (the ⌘/Ctrl+Enter chord) parks the message even on a harness that steers. */
  async function send({ queue = false }: { queue?: boolean } = {}) {
    // Slash tokens stay in the wire form: the server resolves every selected
    // skill and supplies this exact message as their shared request context.
    const originalText = draft.trim();
    const planCommand = !pendingQuestion
      ? parsePlanCommand(originalText, opts?.planActivation)
      : null;
    const planRequested = !!planCommand;
    const toggledPlanMode = !planActive;
    const independentPlanMode = effectiveCommandPlanMode(
      opts?.planActivation,
      planRequested ? toggledPlanMode : undefined,
      planModeOverrideRef.current,
    );
    const planPermissionMode = planRequested && activeHarness?.id === "claude-code"
      ? toggledPlanMode ? "plan" : "auto"
      : undefined;
    const text = planCommand ? planCommand.prompt : originalText;
    const pending = attachments;
    const pendingAnnotations = annotations;
    const wireAnnotations = pendingAnnotations.map((annotation) => ({
      text: annotation.text,
    }));
    const sourceProjectId = projectId;
    let sourceSessionId = activeId;
    const inSourceScope = () => {
      const current = composerScopeRef.current;
      return current.projectId === sourceProjectId && current.activeId === sourceSessionId;
    };
    const restoreComposer = () => {
      if (!inSourceScope()) return;
      setDraft((current) => current || originalText);
      setAttachments((current) => (current.length ? current : pending));
      setAnnotations((current) => (current.length ? current : pendingAnnotations));
    };
    if (planRequested && !text && pending.length === 0 && pendingAnnotations.length === 0) {
      setDraft("");
      setSkillMenuDismissed(false);
      try {
        if (activeHarness?.id === "claude-code") {
          setPermissionMode(toggledPlanMode ? "plan" : "auto");
        } else if (opts?.planActivation === "command") {
          await setIndependentPlanMode(toggledPlanMode);
        } else {
          throw new Error(m.chat_selected_harness_unavailable());
        }
      } catch {
        setSettingsError(m.chat_toggle_plan_failed());
        restoreComposer();
      }
      return;
    }
    const effective = composerSelection
      ? {
          ...composerSelection,
          ...(planPermissionMode ? { permissionMode: planPermissionMode } : {}),
        }
      : null;
    if (planPermissionMode) setPermissionMode(planPermissionMode);
    let planCommandMutation: number | null = null;
    const previousPlanModeOverride = planModeOverrideRef.current;
    if (planRequested && opts?.planActivation === "command") {
      planCommandMutation = ++planMutationSeq.current;
      planModeOverrideRef.current = toggledPlanMode;
      setPlanModeOverride(toggledPlanMode);
    }
    const clearFailedPlanCommand = () => {
      if (planCommandMutation === null || planMutationSeq.current !== planCommandMutation) return;
      planModeOverrideRef.current = previousPlanModeOverride;
      setPlanModeOverride(previousPlanModeOverride);
    };
    if (!text && pending.length === 0 && pendingAnnotations.length === 0) return;
    // A pending question card owns plain typed text as a custom answer
    // (Claude-desktop behavior). This also works while the turn is HELD on
    // the card — where a new message would be rejected as busy and silently
    // dropped. A failed answer restores the draft so the text isn't lost.
    // (Nothing is chipped or expanded while a card is pending — a `/command`
    // in the answer serializes into the note text exactly as it reads.)
    if ((text || pendingAnnotations.length > 0) && pendingQuestion && pending.length === 0) {
      setDraft("");
      setAnnotations([]);
      void respond({
        promptId: pendingQuestion,
        answers: [],
        note: text || undefined,
        annotations: wireAnnotations,
      }).then((ok) => {
        if (!ok) restoreComposer();
      });
      return;
    }
    const turnSignature = JSON.stringify({
      text,
      images: pending.map((attachment) => ({
        mediaType: attachment.mediaType,
        name: attachment.name,
        dataUrl: attachment.dataUrl,
      })),
      annotations: wireAnnotations,
      settings: effective
        ? {
            model: effective.model,
            serviceTier: effective.serviceTier,
            permissionMode: effective.permissionMode,
            planMode: independentPlanMode,
            reasoningLevel: effective.reasoningLevel,
          }
        : null,
    });
    const clientTurnId = pendingClientTurn.current?.signature === turnSignature
      ? pendingClientTurn.current.id
      : `ct_${crypto.randomUUID()}`;
    pendingClientTurn.current = { signature: turnSignature, id: clientTurnId };
    if (busy) {
      // A turn is already running. Steering hands the message to it now, and
      // the delivered text comes back inline on the assistant message. Parking
      // (no steering support, or the queue chord) instead runs it when the turn
      // ends: the server enqueues it and echoes chat.queued to render the chip
      // — no optimistic transcript bubble, since it hasn't run yet.
      if (!activeId || !activeHarness?.agentReady) {
        clearFailedPlanCommand();
        return;
      }
      const sid = activeId;
      setDraft("");
      setAttachments([]);
      setAnnotations([]);
      setAttachError(null);
      // Always send the composer's settings, steer or not: a permission or
      // plan change persists itself before this message, so the server's
      // comparison against the *running* turn is the only thing that catches
      // it — and a mismatch parks the message, which also persists the change.
      const turnOpts = effective
        ? {
            model: effective.model,
            serviceTier: effective.serviceTier,
            permissionMode: effective.permissionMode,
            // The composer's plan state, not just an unpersisted toggle: a
            // toggle that already persisted would otherwise reach the server
            // as "no change" and steer into a turn still running without it.
            planMode:
              opts?.planActivation === "command"
                ? (independentPlanMode ?? openSession?.planMode)
                : independentPlanMode,
            reasoningLevel: effective.reasoningLevel,
          }
        : {};
      setSessionOverride({});
      const images: ChatImageAttachment[] = pending.map((a) => ({
        mediaType: a.mediaType,
        dataBase64: a.dataUrl.slice(a.dataUrl.indexOf(",") + 1),
        name: a.name,
      }));
      try {
        const sendBusy = () =>
          sendChatMessage(
            sid,
            text,
            turnOpts,
            images.length ? images : undefined,
            wireAnnotations,
            clientTurnId,
            steering && !queue && !planRequested ? "steer" : undefined,
          );
        const response = await queueSessionMutation(sendBusy);
        if (response.turn?.existing) await reseedSession(sid);
        setRecoveryOverrides({});
        if (pendingClientTurn.current?.id === clientTurnId) pendingClientTurn.current = null;
      } catch {
        // Never reached the turn — restore the composer so a retry is one keypress.
        clearFailedPlanCommand();
        restoreComposer();
      }
      return;
    }
    if (!activeHarness?.agentReady) {
      clearFailedPlanCommand();
      return;
    }
    // `composerSelection` already resolves to the open session's settings (+ any
    // unsent tweak) or, for a new session, the global preference.
    if (!effective) {
      clearFailedPlanCommand();
      return;
    }
    setDraft("");
    setAttachments([]);
    setAnnotations([]);
    setAttachError(null);
    let sid = activeId;
    try {
      if (!sid) {
        const session = await createChatSession(projectId, effective.harness, {
          model: effective.model,
          serviceTier: effective.serviceTier,
          permissionMode: effective.permissionMode,
          planMode: independentPlanMode,
          reasoningLevel: effective.reasoningLevel,
        });
        loadedSessions.current.add(session.id);
        setSessions((cur) => [session, ...cur]);
        setActiveId(session.id);
        sid = session.id;
        sourceSessionId = session.id;
        composerScopeRef.current = { projectId, activeId: session.id };
      }
      dispatch({
        type: "optimisticUser",
        sessionId: sid,
        text: text || m.chat_asked_about_selection(),
        attachments: pending.map((a) => ({ url: a.dataUrl, mediaType: a.mediaType, name: a.name })),
        annotations: pendingAnnotations,
      });
      dispatch({ type: "busy", sessionId: sid, busy: true });
      pinTranscriptToBottom();
      // The session being sent to is never archived after this turn (new ones
      // start active; existing ones are unarchived server-side by activity) —
      // leave the Archived-only view so its row stays visible.
      if (sessionFilter === "archived") setSessionFilter("active");
      // `effective.harness` is always the target session's harness (locked once
      // it exists), so these overrides are always valid — the backend persists
      // them as the session's sticky settings. Clear the unsent tweak now.
      const turnOpts = effective
        ? {
            model: effective.model,
            serviceTier: effective.serviceTier,
            permissionMode: effective.permissionMode,
            planMode: independentPlanMode,
            reasoningLevel: effective.reasoningLevel,
          }
        : {};
      setSessionOverride({});
      const images: ChatImageAttachment[] = pending.map((a) => ({
        mediaType: a.mediaType,
        dataBase64: a.dataUrl.slice(a.dataUrl.indexOf(",") + 1),
        name: a.name,
      }));
      const targetSessionId = sid;
      if (!targetSessionId) throw new Error(m.chat_session_not_created());
      const sendTurn = () =>
        sendChatMessage(
          targetSessionId,
          text,
          turnOpts,
          images.length ? images : undefined,
          wireAnnotations,
          clientTurnId,
        );
      const response = await queueSessionMutation(sendTurn);
      if (response.turn?.existing) await reseedSession(targetSessionId);
      setRecoveryOverrides({});
      if (pendingClientTurn.current?.id === clientTurnId) pendingClientTurn.current = null;
    } catch (err) {
      // The message never reached a turn — put it back in the composer so a
      // retry is one keypress, whichever branch below applies.
      restoreComposer();
      clearFailedPlanCommand();
      if (!sid) return; // session creation failed; no transcript to annotate
      const msg = err instanceof Error ? err.message : String(err);
      // A *network* failure does not prove no turn started — the backend
      // claims the turn (and emits busy) before its response, so a lost
      // response can reject on a live, streaming turn; ask the server before
      // declaring failure. An explicit busy rejection is different: the slot
      // belongs to someone else's turn (run watcher, second tab) and ours was
      // never accepted — always surface that.
      if (!/session is busy/i.test(msg)) {
        const busyNow = await listChatSessions(projectId)
          .then((list) => !!list.find((s) => s.id === sid)?.busy)
          .catch(() => false);
        if (busyNow) {
          // The turn is real and streaming — undo the restore, nothing failed.
          if (inSourceScope()) {
            setDraft((cur) => (cur === text ? "" : cur));
            setAttachments((cur) => (cur === pending ? [] : cur));
            setAnnotations((cur) => (cur === pendingAnnotations ? [] : cur));
          }
          return;
        }
      }
      dispatch({ type: "busy", sessionId: sid, busy: false });
      dispatch({ type: "localError", sessionId: sid, text: m.chat_message_not_sent({ error: ltr(msg) }) });
    }
  }

  function stop() {
    if (!activeId) return;
    void interruptChat(activeId).catch(() => {
      setSettingsError(m.chat_stop_failed());
    });
  }

  const recoverFailedTurn = useCallback(
    async (turnId: string, action: "retry" | "continue") => {
      if (!activeId || recoveringTurnRef.current) return;
      recoveringTurnRef.current = true;
      setSettingsError(null);
      setRecoveringTurnId(turnId);
      try {
        const turnOpts = recoveryTurnOptions({
          model: recoveryOverrides.model,
          serviceTier: recoveryOverrides.serviceTier,
          permissionMode: recoveryOverrides.permissionMode,
          planMode: recoveryOverrides.planMode,
          reasoningLevel: recoveryOverrides.reasoningLevel,
        });
        const sessionId = activeId;
        const response = await recoverChatTurn(sessionId, turnId, action, turnOpts);
        if (response.turn.existing) await reseedSession(sessionId);
        setRecoveryOverrides({});
      } catch {
        setSettingsError(m.chat_recover_failed());
      } finally {
        recoveringTurnRef.current = false;
        setRecoveringTurnId(null);
      }
    },
    [activeId, recoveryOverrides, reseedSession],
  );

  const forkTurn = useCallback(
    (messageId: string, text: string) => {
      if (!activeId || busy || !activeHarness?.agentReady) return;
      const sid = activeId;
      dispatch({ type: "busy", sessionId: sid, busy: true });
      pinTranscriptToBottom();
      void queueSessionMutation(() => forkChatTurn(sid, messageId, text)).catch((err) => {
        dispatch({ type: "busy", sessionId: sid, busy: false });
        const detail = err instanceof Error ? err.message : String(err);
        dispatch({ type: "localError", sessionId: sid, text: m.chat_resend_failed({ error: ltr(detail) }) });
      });
    },
    [activeId, busy, activeHarness?.agentReady, pinTranscriptToBottom, queueSessionMutation],
  );

  /** Show a different fork. The server descends to that branch's newest tip and
   * echoes the real leaf back over chat.branch. */
  const selectBranch = useCallback(
    (leafId: string) => {
      // Moving the branch pointer out from under a running turn would land that
      // turn's reply on whichever branch the pointer stopped at.
      if (!activeId || busy) return;
      const sid = activeId;
      // Via the ref, not a dep: the leaf changes on every new message, and this
      // callback reaches every row through the memoized `Message`.
      const previous = activeLeafRef.current;
      dispatch({ type: "activeLeaf", sessionId: sid, leafId });
      void queueSessionMutation(() => selectChatBranch(sid, leafId)).catch((err) => {
        // Leaving the client on a branch the server did not switch to would send
        // the next message somewhere other than what is on screen.
        dispatch({ type: "activeLeaf", sessionId: sid, leafId: previous });
        const detail = err instanceof Error ? err.message : String(err);
        dispatch({ type: "localError", sessionId: sid, text: m.chat_switch_fork_failed({ error: ltr(detail) }) });
      });
    },
    [activeId, busy, queueSessionMutation],
  );

  // Durable cancellation wins before the chip disappears. If persistence
  // fails, leave it visible so a restart cannot surprise the user by sending it.
  function cancelQueued(itemId: string) {
    if (!activeId) return;
    const sid = activeId;
    void cancelQueuedMessage(sid, itemId)
      .then(({ removed }) => {
        if (!removed) return;
        return reseedSession(sid);
      })
      .catch(() => setSettingsError(m.chat_remove_queued_failed()));
  }

  async function retryQueued(itemId: string) {
    if (!activeId || retryingQueuedId) return;
    const sid = activeId;
    setSettingsError(null);
    setRetryingQueuedId(itemId);
    try {
      await retryQueuedMessage(sid, itemId);
      await reseedSession(sid);
    } catch {
      setSettingsError(m.chat_retry_queued_failed());
    } finally {
      setRetryingQueuedId(null);
    }
  }

  // Escape stops the streaming turn and drops focus back into the composer,
  // mirroring the Claude Code desktop app. Harness-agnostic — `stop()` →
  // `interruptChat` interrupts whichever harness (Claude, Codex, OpenCode, …)
  // is running the active session. Only armed while chat is visible.
  //
  // An overlay that should swallow Escape (rather than let it stop the turn)
  // must own the key ahead of this document-level bubble listener, by one of
  // two means already in use — a new overlay has to pick one or it will
  // interrupt the turn on Escape:
  //   - the slash menu preventDefaults in the composer's onKeyDown (bubble),
  //     so the `defaultPrevented` guard below defers to it;
  //   - the composer pickers (usePopover) stopPropagation in the capture phase,
  //     so their Escape never reaches this listener at all.
  useEffect(() => {
    if (!busy || mainView !== "chat") return;
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      stop();
      composerRef.current?.focus();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [busy, activeId, mainView]);

  /** Drop every trace of a session — the local row, the open-thread selection,
   * and the cached transcript. Used on delete (ours or another dashboard's). */
  function forgetSession(sessionId: string) {
    deletedIds.current.add(sessionId);
    setSessions((cur) => cur.filter((s) => s.id !== sessionId));
    setActiveId((cur) => (cur === sessionId ? null : cur));
    setUnreadSessionIds((current) => {
      if (!current.has(sessionId)) return current;
      const next = new Set(current);
      next.delete(sessionId);
      return next;
    });
    loadedSessions.current.delete(sessionId);
    seenTitles.current.delete(sessionId);
    dispatch({ type: "forget", sessionId });
  }

  function setArchived(session: ChatSession, archived: boolean) {
    // Optimistic; the server also broadcasts the row over chat.session. On
    // failure restore the pre-request snapshot (not the request's negation,
    // which could undo a concurrent authoritative update).
    const prev = session.archived;
    setSessions((cur) => cur.map((s) => (s.id === session.id ? { ...s, archived } : s)));
    // Deselect only when the row leaves the rail's current filter — keeping it
    // selected would leave the thread (and Agent-scoped panes) keyed to an
    // invisible session. Kept even if the request fails; it's a no-op then.
    if (!matchesFilter(sessionFilter, archived))
      setActiveId((cur) => (cur === session.id ? null : cur));
    void setChatSessionArchived(session.id, archived).catch(() => {
      setSessions((cur) =>
        cur.map((s) => (s.id === session.id ? { ...s, archived: prev } : s)),
      );
    });
  }

  function rename(session: ChatSession, title: string) {
    // Optimistic; the server trims and re-broadcasts the row over chat.session.
    // On failure restore the pre-request title (not the draft) so a concurrent
    // authoritative update isn't undone.
    const prev = session.title;
    setSessions((cur) => cur.map((s) => (s.id === session.id ? { ...s, title } : s)));
    void renameChatSession(session.id, title).catch(() => {
      setSessions((cur) => cur.map((s) => (s.id === session.id ? { ...s, title: prev } : s)));
    });
  }

  async function removeSession(session: ChatSession) {
    const title = session.title?.trim() || m.chat_untitled();
    if (!window.confirm(m.chat_delete_session_confirm({ title: autoDir(title) }))) return;
    try {
      await deleteChatSession(session.id);
    } catch (err) {
      showAlert(m.chat_delete_session_failed({ title: autoDir(title), error: ltr(err instanceof Error ? err.message : String(err)) }), "error");
      return;
    }
    forgetSession(session.id);
  }

  /** Deliver a card answer; resolves `false` when delivery failed (so a
   * caller can e.g. restore a consumed draft). Stable per session so the
   * memoized Message rows don't re-render on unrelated state changes. */
  const respond = useCallback(
    (answer: PromptAnswer): Promise<boolean> => {
      if (!activeId) return Promise.resolve(false);
      const sid = activeId;
      // The resumed turn streams over SSE; optimistically mark busy.
      dispatch({ type: "busy", sessionId: sid, busy: true });
      return queueSessionMutation(() => respondChat(sid, answer))
        .then(() => true)
        .catch(() => false)
        .finally(() => {
          // Reconcile with the store: if this tab's copy of the card was stale
          // (e.g. the held turn timed out and resolved it while our SSE was
          // dropped), the answer no-ops server-side and nothing re-broadcasts —
          // without this the card stays actionable forever and every answer
          // silently dead-ends. Busy is reconciled from the server for THIS
          // session only (a whole-set replace could stomp another session's
          // just-started optimistic flag), so the optimistic dispatch above
          // can't wedge true after a no-op or failure.
          getChatMessages(sid)
            .then(({ messages, queued, activeLeafId }) =>
              dispatch({ type: "seed", sessionId: sid, messages, queued, activeLeafId }),
            )
            .catch(() => {});
          listChatSessions(projectId)
            .then((list) =>
              dispatch({
                type: "busy",
                sessionId: sid,
                busy: !!list.find((s) => s.id === sid)?.busy,
              }),
            )
            // On a failed fetch keep the optimistic flag: clearing busy while a
            // Handled resume is still streaming would hide Working…/Stop for
            // the rest of the turn (nothing re-asserts busy mid-stream).
            .catch(() => {});
        });
    },
    [activeId, projectId, queueSessionMutation],
  );

  const visibleSessions = sessions.filter((s) => matchesFilter(sessionFilter, s.archived));
  const isApple = /Mac|iPhone|iPad/.test(navigator.platform);
  const newTaskShortcut = isApple ? "⌘ ⇧ Enter" : "Ctrl + Shift + Enter";
  const queueChord = isApple ? "⌘ Enter" : "Ctrl + Enter";
  const startNewTask = useCallback(() => {
    setSessionFilter("active");
    setActiveId(null);
    onSelectMainView("chat");
  }, [onSelectMainView]);

  /** Follow a spawn card into the session it started. Spawned sessions are
   * ordinary top-level sessions, so this is just a switch in the rail — via
   * "All", because selecting a row the active filter hides would leave the
   * thread keyed to a session with no row (see `setArchived`). */
  const openSpawnedSession = useCallback(
    (sessionId: string) => {
      setSessionFilter("all");
      setActiveId(sessionId);
      onSelectMainView("chat");
    },
    [onSelectMainView],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.repeat ||
        event.key !== "Enter" ||
        (!event.metaKey && !event.ctrlKey) ||
        event.altKey ||
        !event.shiftKey
      )
        return;
      event.preventDefault();
      startNewTask();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [startNewTask]);

  const rail = (
    <aside className="session-rail w-68 shrink-0 flex flex-col mt-5 me-3.5 mb-5 ms-0 bg-background min-h-0 [&_.rail-body]:flex-1 [&_.rail-body]:min-h-0 [&_.rail-body]:overflow-y-auto [&_.rail-body]:py-1 [&_.rail-body]:px-2 border border-border rounded-lg overflow-visible shadow-elevated">
      {railHeader}
      {/* Workspace tools open beside chat; settings sections replace the middle pane. */}
      <nav className="rail-nav flex flex-col gap-0.5 p-2 shrink-0">
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-start [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium ${filesActive ? "active" : ""}`}
          onClick={onOpenWorktree}
        >
          <FolderOpen size={15} />
          {m.chat_panel_files()}
        </button>
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-start [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium ${artifactsActive ? "active" : ""}`}
          data-onboarding="nav-artifacts"
          onClick={onOpenArtifacts}
        >
          <Package size={15} />
          {m.chat_panel_artifacts()}
        </button>
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-start [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium ${experimentsActive ? "active" : ""}`}
          onClick={onOpenExperiments}
        >
          <FlaskConical size={15} />
          {m.chat_panel_experiments()}
        </button>
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-start [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium ${mainView === "skills" ? "active" : ""}`}
          onClick={() => onSelectMainView("skills")}
        >
          <Blocks size={15} />
          {m.chat_panel_customize()}
        </button>
        {SETTINGS_NAV.map((item) => (
          <button
            key={item.id}
            className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-start [&:hover:not(.active)]:bg-surface [&.active]:bg-panel [&.active]:font-medium ${mainView !== "chat" && mainView !== "skills" && item.activeTabs.includes(mainView) ? "active" : ""}`}
            data-onboarding={item.id === "compute" ? "nav-compute" : undefined}
            onClick={() => onSelectMainView(item.id)}
          >
            {item.icon}
            {item.label()}
          </button>
        ))}
      </nav>
      <div className="rail-section-head flex items-center justify-between shrink-0 pt-3.5 pe-2.5 pb-1.5 ps-4.5">
        <div className="rail-section-label p-0 text-sm font-medium text-subtext">
          {SESSION_FILTERS.find((f) => f.id === sessionFilter)?.railLabel() ?? m.chat_recents()}
        </div>
        <div className="rail-section-actions flex items-center gap-0.5">
          <button
            className="rail-section-new inline-flex items-center gap-1 py-[3px] px-1.5 rounded-sm text-subtext text-sm font-medium [&:hover]:text-text [&:hover]:bg-surface tip-up [&[data-tip]::after]:top-auto [&[data-tip]::after]:bottom-[calc(100%_+_6px)]"
            data-onboarding="new-session"
            data-tip={newTaskShortcut}
            aria-keyshortcuts="Meta+Shift+Enter Control+Shift+Enter"
            onClick={startNewTask}
          >
            <Plus size={13} />
            {m.chat_panel_task()}
          </button>
          <SessionFilterMenu value={sessionFilter} onChange={setSessionFilter} />
        </div>
      </div>
      <div className="rail-body">
        {visibleSessions.map((s) => (
          <SessionRow
            key={s.id}
            session={s}
            active={s.id === activeId && mainView === "chat"}
            unread={unreadSessionIds.has(s.id)}
            busy={state.busySessions.has(s.id)}
            waiting={waitingSessions.has(s.id)}
            revealTitle={titleReveals.get(s.id)}
            onOpen={() => {
              setActiveId(s.id);
              if (projectId === DEMO_PROJECT_ID) markDemoSessionRead(s.id);
              setUnreadSessionIds((current) => {
                if (!current.has(s.id)) return current;
                const next = new Set(current);
                next.delete(s.id);
                return next;
              });
              onSelectMainView("chat");
            }}
            onRename={(title) => rename(s, title)}
            onSetArchived={(archived) => setArchived(s, archived)}
            onDelete={() => void removeSession(s)}
         />
        ))}
        {visibleSessions.length === 0 && (
          <div className="rail-empty py-1.5 px-2.5 text-sm text-muted">
            {sessionFilter === "archived"
              ? m.chat_no_archived_sessions()
              : sessions.length > 0
                ? m.chat_no_active_sessions()
                : m.chat_no_sessions_yet()}
          </div>
        )}
      </div>
      {runtime.kind === "ssh" ? (
        <RemoteStatus runtime={runtime} />
      ) : (
        <div className="relative shrink-0 border-t border-border">
          <div className="flex items-center gap-1.5 px-1 py-2">
            <IconButton size="small" aria-label={m.remote_dialog_title()} aria-haspopup="dialog" onClick={() => setRemoteDialogOpen(true)}>
              <RemoteIcon size={14} className="shrink-0" />
            </IconButton>
            <span className="flex min-w-0 flex-col gap-1 text-start text-text">
              <span className="truncate text-sm leading-tight">{m.projects_local()}</span>
              <span className="truncate text-xs leading-tight text-subtext">OpenResearch {ltr(runtime.version)}</span>
            </span>
          </div>
        </div>
      )}
      {remoteDialogOpen && (
        <RemoteHostDialog
          onClose={() => setRemoteDialogOpen(false)}
          onConfigureSsh={() => {
            setRemoteDialogOpen(false);
            setSshConfigOpen(true);
          }}
        />
      )}
      {sshConfigOpen && (
        <SshConfigDialog
          onClose={() => {
            setSshConfigOpen(false);
            setRemoteDialogOpen(true);
          }}
        />
      )}
    </aside>
  );

  // With the rail hidden, the header stretches to the full pane width
  // (Claude-desktop style): the reopen toggle sits in the window's top-left
  // corner with the title beside it, instead of riding the centered readable
  // column.
  const headerClass = `chat-header flex items-center gap-2 py-0 px-4 bg-background shrink-0 h-12 relative z-4 w-full max-w-readable my-0 mx-auto [&.rail-hidden]:max-w-none [&.rail-hidden]:py-0 [&.rail-hidden]:px-0.5 [&::after]:content-[''] [&::after]:absolute [&::after]:top-full [&::after]:start-0 [&::after]:end-0 [&::after]:h-6 [&::after]:bg-[linear-gradient(to_bottom,_var(--base),_transparent)] [&::after]:pointer-events-none${railOpen ? "" : " rail-hidden"}`;
  const railReopen = !railOpen && (
    <IconButton
      title={m.chat_panel_show_sidebar()}
      aria-label={m.chat_panel_show_sidebar()}
      onClick={onShowRail}
    >
      <PanelLeft size={15} />
    </IconButton>
  );

  if (mainView !== "chat") {
    return (
      <>
        {railOpen && rail}
        <section className="chat-pane flex-1 min-w-0 flex flex-col bg-background min-h-0">
          {!railOpen && <div className={headerClass}>{railReopen}</div>}
          <div className="settings-view-scroll flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges]">{children}</div>
        </section>
      </>
    );
  }

  return (
    <>
      {railOpen && rail}
      <section className="chat-pane flex-1 min-w-0 flex flex-col bg-background min-h-0">
      {/* Header — session title on the left, end-pane view switchers on the
          right, fading into the chat below (sessions live in the rail). */}
      <div className={headerClass}>
        {railReopen}
        <PaperTitle variant="header"
          title={activeSession ? activeSession.title?.trim() || m.chat_untitled() : m.chat_new_session()}
        >
          {activeSession ? (
            <TitleReveal
              key={activeTitleReveal ?? "static"}
              title={activeSession.title?.trim() || m.chat_untitled()}
              animate={activeTitleReveal !== undefined}
           />
          ) : (
            m.chat_new_session()
          )}
        </PaperTitle>
        {onOpenDemoWelcome && (
          <IconButton
            data-tip={m.chat_panel_about_this_demo()}
            aria-label={m.chat_panel_about_this_demo()}
            onClick={onOpenDemoWelcome}
          >
            <HelpCircle size={15} />
          </IconButton>
        )}
      </div>

      {historyLoading ? (
        <div className="chat-loading flex-1 flex items-center justify-center gap-3 text-subtext text-xl p-5 [&_.spinner]:w-5.5 [&_.spinner]:h-5.5 [&_.spinner]:border-[3px]" aria-live="polite" aria-busy="true">
          <Spinner />
          <span>{m.chat_panel_loading_conversation()}</span>
        </div>
      ) : !threadMounted ? (
        <div className="chat-empty flex-1 flex flex-col items-center justify-center text-text p-8 text-center [&_h2]:m-0 [&_h2]:text-5xl [&_h2]:font-medium [&_h2]:tracking-[-0.015em] [&_h2]:text-text">
          <div className="chat-empty-mark w-10.5 h-10.5 mb-5.5 [&_svg]:block [&_svg]:w-full [&_svg]:h-full">
            <BrandMark />
          </div>
          <h2>{m.chat_panel_what_should_we_research()}</h2>
          <div className="chat-empty-project inline-flex items-center gap-[7px] mt-3 py-1.5 px-3 border border-border rounded-full text-subtext bg-surface text-lg font-medium">
            <FolderOpen size={19} />
            <span>{projectName}</span>
          </div>
          {starterLoading && (
            <div
              className={STARTER_GRID_CLASS}
              role="status"
              aria-live="polite"
              aria-label={m.chat_panel_starter_generating()}
              aria-busy="true"
            >
              {STARTER_ICONS.map((Icon, index) => (
                <div
                  key={index}
                  className={`flex min-h-22 animate-pulse flex-col items-start justify-center gap-2.5 rounded-xl border bg-background px-5 py-4 ${STARTER_TONES[index].box}`}
                >
                  <span className={`flex w-full items-center gap-2.5 ${STARTER_TONES[index].icon}`}>
                    <Icon size={17} />
                    <span className="h-3.5 w-2/5 rounded bg-surface-bright" />
                  </span>
                  <span className="h-3 w-4/5 rounded bg-surface" />
                </div>
              ))}
            </div>
          )}
          {starterPrompts && (
            <div className={STARTER_GRID_CLASS} role="group" aria-label={m.chat_panel_starter_prompts()}>
              {starterPrompts.map((item, index) => {
                const Icon = STARTER_ICONS[index];
                const tone = STARTER_TONES[index];
                return (
                  <button
                    key={index}
                    type="button"
                    className={`flex min-h-22 w-full min-w-0 cursor-pointer flex-col items-start justify-center gap-1.5 rounded-xl border bg-background px-5 py-4 text-start font-sans transition-colors duration-120 ease-standard hover:bg-surface ${tone.box}`}
                    onClick={() => applyStarterPrompt(item.prompt)}
                  >
                    <span className="flex items-center gap-2.5 text-base font-medium text-text">
                      <Icon size={17} className={tone.icon} />
                      {item.title}
                    </span>
                    <span className="w-full truncate text-sm text-subtext">{item.prompt}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      ) : (
        <div
          className="chat-thread flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges]"
          ref={threadRef}
          onScroll={(e) => {
            updateTranscriptBottom(e.currentTarget);
            transcriptSelection.dismiss();
          }}
        >
          <div className="chat-thread-inner max-w-readable my-0 mx-auto pt-4 px-4 pb-8 flex flex-col gap-4" ref={threadInnerRef}>
            <Transcript
              messages={messages}
              allMessages={allMessages}
              canFork={canFork}
              onFork={forkTurn}
              onSelectFork={selectBranch}
              busy={busy}
              onOpenFile={openFileInSession}
              onOpenRun={onOpenRun}
              onOpenSpawnedSession={openSpawnedSession}
              runExperimentName={runExperimentName}
              onOpenExperiment={onOpenExperiment}
              experimentName={experimentName}
              onRespond={respond}
              onOpenPlan={openPlan}
              onOpenSubagent={openSubagent}
              recoveringTurnId={recoveringTurnId}
              onRecover={recoverFailedTurn}
              skills={commands}
           />
            {busy && awaitingInput && (
              <div className="flex items-center gap-2 text-subtext text-sm pt-0.5 px-0 pb-2 italic">{m.chat_panel_waiting_for_your_input()}</div>
            )}
            {busy && !awaitingInput && !hasPendingTailTool && !hasStreamingText && (
              <div className="text-base pt-0.5 px-1 pb-2">
                <span className="tool-running-shimmer">{m.chat_thinking()}</span>
              </div>
            )}
          </div>
        </div>
      )}

      {transcriptSelection.action && (
        <Button
          type="button"
          size="small"
          className="chat-selection-action fixed z-50 shadow-control"
          style={{
            left: transcriptSelection.action.x,
            top: transcriptSelection.action.top,
            transform: "translateX(-50%)",
          }}
          onMouseDown={(event) => event.preventDefault()}
          onClick={transcriptSelection.add}
        >
          <MessageSquareQuote size={14} />
          {m.chat_panel_ask_about_this()}
        </Button>
      )}

      {/* Docked while a plan awaits a decision, so the approval controls never
          scroll away. Actions mirror the (now compact) inline card's wire. */}
      <div className="composer px-3 pb-5 shrink-0 relative z-4 bg-background w-full max-w-readable my-0 mx-auto [&_textarea]:border-0 [&_textarea]:bg-none [&_textarea]:bg-transparent [&_textarea]:resize-none [&_textarea]:pt-2.5 [&_textarea]:px-3 [&_textarea]:pb-1 [&_textarea]:text-base [&_textarea]:field-sizing-content [&_textarea]:min-h-18 [&_textarea]:max-h-45">
        {threadMounted && (
          <IconButton
            className={`absolute bottom-full left-1/2 z-5 mb-6 h-9 w-9 -translate-x-1/2 rounded-full border border-border bg-background shadow-control transition-opacity duration-150 ease-standard ${transcriptAtBottom ? "opacity-0" : "opacity-100"}`}
            title={m.chat_scroll_to_bottom()}
            aria-label={m.chat_scroll_to_bottom()}
            inert={transcriptAtBottom}
            onClick={scrollToTranscriptBottom}
          >
            {busy && !awaitingInput ? (
              <MoreHorizontal size={18} className="tool-running-shimmer-icon" />
            ) : (
              <ArrowDown size={16} />
            )}
          </IconButton>
        )}
        {/* Inside the composer so the composer's popovers (mode/model pickers,
            z 50 within this stacking context) layer above the strip — as a
            sibling, the composer's own z-index: 4 capped them below it. */}
        {/* Hidden while a submitted revision is in flight so the outgoing
            card never sits there looking actionable; the revised card swaps
            in when it arrives (effect above). The transcript status covers
            the interim ("Waiting for your input…" for a beat until the old
            card's resolve broadcast lands, then Working…). */}
        {pendingPlan && !(revisingPlan && pendingPlan.promptId === revisingPlan.promptId) && (
          <PlanStrip
            synthesized={pendingPlan.synthesized}
            agentLabel={
              activeSession ? HARNESS_LABELS[activeSession.harness] : m.chat_the_agent()
            }
            showResumeModes={activeSession?.harness === "claude-code"}
            onView={(intent) => openPlan?.(pendingPlan.plan, pendingPlan.promptId, intent)}
            onApprove={(resumeMode) =>
              respond({
                promptId: pendingPlan.promptId,
                approve: true,
                ...(resumeMode ? { resumeMode } : {}),
              })
            }
            // Plain rejection — no note; the model stops and waits.
            onReject={() => respond({ promptId: pendingPlan.promptId, approve: false })}
            // The strip owns its own revise textarea (Claude-desktop style);
            // the note comes back on submit, always non-empty (note presence
            // is what distinguishes revise from reject on the wire).
            onRevise={(note) => {
              if (activeId) setRevising({ sessionId: activeId, promptId: pendingPlan.promptId });
              respond({ promptId: pendingPlan.promptId, approve: false, note });
            }}
         />
        )}
        {queued.length > 0 && (
          <div className="composer-queued flex flex-col gap-1 mb-1.5">
            {queued.map((q, index) => (
              <div
                key={q.id}
                className="queued-chip flex flex-wrap items-center gap-x-2 gap-y-1 py-1.5 px-2.5 text-sm text-subtext bg-background border border-border rounded-sm"
                title={q.error ? `${q.text}\n\n${q.error}` : q.text}
              >
                {q.dispatchState === "blocked"
                  ? <TriangleAlert size={13} className="shrink-0 text-accent-amber" />
                  : <Clock size={13} className="shrink-0 text-muted" />}
                <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-text">
                  {q.text}
                </span>
                {q.dispatchState !== "blocked" && (
                  <span className="shrink-0 text-sm text-muted">
                    {q.dispatchState === "retrying"
                      ? queuedRetryLabel(q.nextRetryAt, queueClock)
                      : m.chat_queued()}
                  </span>
                )}
                {q.dispatchState === "blocked" ? (
                  <>
                    <button
                      onClick={() => void retryQueued(q.id)}
                      aria-label={m.a11y_retry_queued_message({ text: q.text })}
                      disabled={retryingQueuedId !== null}
                      className="shrink-0 px-1.5 py-0.5 border border-border rounded-sm text-sm text-text bg-background cursor-pointer disabled:opacity-50 disabled:cursor-default [&:hover:not(:disabled)]:border-text"
                    >
                      {retryingQueuedId === q.id ? m.retrying() : m.app_retry()}
                    </button>
                    <button
                      onClick={() => cancelQueued(q.id)}
                      aria-label={m.a11y_remove_queued_message({ text: q.text })}
                      disabled={retryingQueuedId !== null}
                      className="shrink-0 px-1.5 py-0.5 border-0 text-sm text-muted bg-transparent cursor-pointer disabled:opacity-50 disabled:cursor-default [&:hover:not(:disabled)]:text-text"
                    >
                      {m.chat_panel_remove()}
                    </button>
                    {index === firstBlockedQueueIndex && index < queued.length - 1 && (
                      <span className="basis-full ps-5 text-sm text-muted">
                        {m.chat_panel_later_queued_messages_will_wait_until_this_is()}
                      </span>
                    )}
                  </>
                ) : (
                  <button
                    title={m.chat_panel_remove_queued_message()}
                    aria-label={m.chat_panel_remove_queued_message()}
                    onClick={() => cancelQueued(q.id)}
                    className="shrink-0 inline-flex items-center justify-center w-4 h-4 p-0 border-0 rounded-full text-muted cursor-pointer [&:hover]:bg-text [&:hover]:text-background"
                  >
                    <X size={11} />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
        <div className="composer-box relative flex flex-col border border-border rounded-lg bg-background shadow-elevated" data-onboarding="composer">
          {activeHarness && !activeHarness.agentReady && (
            <div className="composer-harness-warning py-2 px-3 text-subtext text-sm leading-normal border-b border-b-border-variant [&_strong]:text-accent-amber [&_strong]:font-medium [&_code]:font-mono [&_code]:text-text">
              <strong>{activeHarness.name} {m.chat_panel_is_unavailable()}</strong>{" "}
              {activeHarness.agentNote ? renderNote(activeHarness.agentNote) : m.chat_recheck_setup()}
            </div>
          )}
          {skillMenuOpen && (
            <SkillMenu
              skills={skillMatches}
              activeIndex={activeSkillIdx}
              onPick={pickSkill}
              onHover={setSkillIdx}
           />
          )}
          {annotations.length > 0 && (
            <ComposerAnnotations
              annotations={annotations}
              onClear={() => {
                setAnnotations([]);
                window.requestAnimationFrame(() => composerRef.current?.focus());
              }}
              onRemove={(id) => {
                const remaining = annotations.filter((annotation) => annotation.id !== id);
                setAnnotations(remaining);
                if (remaining.length === 0) {
                  window.requestAnimationFrame(() => composerRef.current?.focus());
                }
              }}
           />
          )}
          {attachments.length > 0 && (
            <div className="composer-attachments flex flex-wrap gap-1.5 pt-2 px-3 pb-0">
              {attachments.map((a, i) => {
                const remove = () =>
                  setAttachments((cur) => cur.filter((_, j) => j !== i));
                return a.mediaType === "application/pdf" ? (
                  <div key={i} className="attachment-file [&_button]:absolute [&_button]:-top-[5px] [&_button]:-right-[5px] [&_button]:inline-flex [&_button]:items-center [&_button]:justify-center [&_button]:w-4 [&_button]:h-4 [&_button]:p-0 [&_button]:border [&_button]:border-border [&_button]:rounded-full [&_button]:bg-surface [&_button]:text-text [&_button]:cursor-pointer [&_button:hover]:bg-text [&_button:hover]:text-background relative inline-flex items-center gap-2 max-w-55 py-2 px-2.5 border border-border rounded-sm text-text bg-surface [&_svg]:shrink-0 [&_svg]:text-muted" title={a.name}>
                    <FileText size={22} />
                    <span className="attachment-file-name overflow-hidden text-ellipsis whitespace-nowrap text-sm">{a.name ?? "document.pdf"}</span>
                    <button title={m.chat_panel_remove_file()} aria-label={m.chat_panel_remove_file()} onClick={remove}>
                      <X size={11} />
                    </button>
                  </div>
                ) : (
                  <div key={i} className="attachment-thumb relative [&_img]:w-13 [&_img]:h-13 [&_img]:object-cover [&_img]:border [&_img]:border-border [&_img]:rounded-sm [&_img]:block [&_button]:absolute [&_button]:-top-[5px] [&_button]:-right-[5px] [&_button]:inline-flex [&_button]:items-center [&_button]:justify-center [&_button]:w-4 [&_button]:h-4 [&_button]:p-0 [&_button]:border [&_button]:border-border [&_button]:rounded-full [&_button]:bg-surface [&_button]:text-text [&_button]:cursor-pointer [&_button:hover]:bg-text [&_button:hover]:text-background">
                    <img src={a.dataUrl} alt={m.chat_pasted_image()} />
                    <button title={m.chat_panel_remove_image()} aria-label={m.chat_panel_remove_image()} onClick={remove}>
                      <X size={11} />
                    </button>
                  </div>
                );
              })}
            </div>
          )}
          {attachError && (
            <div className="composer-attach-error pt-1.5 px-3 pb-0 text-sm text-accent-red" role="alert">
              {attachError}
            </div>
          )}
          {settingsError && (
            <div className="composer-settings-error pt-1.5 px-3 pb-0 text-sm text-accent-red" role="alert">
              {settingsError}
            </div>
          )}
          <div className="composer-input relative flex overflow-hidden [&_textarea]:flex-1">
            <textarea
              dir="auto"
              ref={composerRef}
              // Native prose stays visible; the aligned mirror paints only skill tokens.
              className="relative z-1 bg-transparent"
              value={draft}
              placeholder={
                // A pending question card owns typed text (see send()); say so.
                // While a steerable turn runs, Enter goes to that turn, so name
                // the gesture and its queue chord — the send button is a Stop
                // button for the whole busy stretch.
                // Otherwise follow `composerSelection` so the name tracks the
                // picker for a new session and the open session once one exists.
                pendingQuestion
                  ? m.chat_type_custom_answer()
                  : steering && activeHarness
                    ? m.chat_steer_placeholder({ harness: ltr(HARNESS_LABELS[activeHarness.id]), shortcut: ltr(queueChord) })
                    : composerSelection
                      ? activeHarness?.agentReady
                        ? m.chat_message_harness({ harness: ltr(HARNESS_LABELS[composerSelection.harness]) })
                        : m.chat_harness_unavailable({ harness: ltr(HARNESS_LABELS[composerSelection.harness]) })
                      : m.chat_ask_agent_placeholder()
              }
              rows={2}
              onPaste={onComposerPaste}
              onDragOver={(e) => {
                if (e.dataTransfer.types.includes("Files")) e.preventDefault();
              }}
              onDrop={(e) => {
                if (e.dataTransfer.files.length === 0) return;
                e.preventDefault();
                addFiles(Array.from(e.dataTransfer.files));
              }}
              onChange={(e) => {
                const v = e.target.value;
                const cursor = e.target.selectionStart;
                setComposerCursor(cursor);
                // `/plan` is the one command the composer consumes rather than
                // sends: it toggles the mode the moment the space lands. Not
                // while a question card is pending (its answer is a note, never
                // a command) and not mid-IME-composition, where the text can
                // transiently look complete.
                const completedCommand =
                  cursor > 0 && /\s/.test(v[cursor - 1]) && !pendingQuestion && !composingRef.current
                    ? slashCommandContext(v, cursor - 1)
                    : null;
                if (completedCommand?.query === "plan" && opts?.planActivation) {
                  activatePlanCommand(v, completedCommand);
                  return;
                }
                const completedSkill = completedCommand
                  ? commands.find(
                      (command) =>
                        command.source !== "command" &&
                        command.name === completedCommand.query,
                    )
                  : undefined;
                if (completedSkill && completedCommand) {
                  const next = insertSlashCommand(v, completedCommand, completedSkill.name, 2);
                  setDraft(next.text);
                  window.requestAnimationFrame(() => {
                    composerRef.current?.setSelectionRange(next.cursor, next.cursor);
                    setComposerCursor(next.cursor);
                  });
                  return;
                }
                setDraft(v);
                setSkillMenuDismissed(false);
              }}
              onSelect={(e) => setComposerCursor(e.currentTarget.selectionStart)}
              onCompositionStart={() => {
                composingRef.current = true;
              }}
              onCompositionEnd={() => {
                composingRef.current = false;
              }}
              onKeyDown={(e) => {
                if (skillMenuOpen) {
                  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                    e.preventDefault();
                    const delta = e.key === "ArrowDown" ? 1 : -1;
                    setSkillIdx(
                      (activeSkillIdx + delta + skillMatches.length) % skillMatches.length,
                    );
                    return;
                  }
                  if (e.key === "Tab" || e.key === "Enter") {
                    e.preventDefault();
                    pickSkill(skillMatches[activeSkillIdx]);
                    return;
                  }
                  if (e.key === "Escape") {
                    e.preventDefault();
                    setSkillMenuDismissed(true);
                    return;
                  }
                }
                // Backspace just behind a chip deletes the whole command.
                // (Escape deliberately doesn't touch it — that's the
                // stop-the-turn gesture, see the document listener above.)
                if (e.key === "Backspace" && deleteCommandBehindCaret(e.currentTarget)) {
                  e.preventDefault();
                  return;
                }
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void send({ queue: e.metaKey || e.ctrlKey });
                }
              }}
           />
            {/* After the textarea: its ref must be attached before the mirror
              * measures it. */}
            <ComposerSkillChips
              text={draft}
              isCommand={knownCommand}
              skills={commands}
              projectId={projectId}
              textareaRef={composerRef}
           />
          </div>
          <div className="composer-actions flex min-w-0 justify-end items-center gap-2 pt-1.5 px-2 pb-2">
            <div className="option-picker relative inline-flex shrink-0" ref={dataSources.ref}>
              <IconButton
                type="button"
                className="composer-bare"
                title={m.chat_panel_data_sources()}
                aria-label={m.chat_panel_data_sources()}
                aria-haspopup="dialog"
                aria-expanded={dataSources.open}
                onClick={() => dataSources.setOpen((open) => !open)}
              >
                <ToggleRight size={16} />
              </IconButton>
              {dataSources.open && (
                <div className="composer-sources-menu absolute bottom-[calc(100%_+_8px)] start-0 z-50 flex min-w-55 flex-col gap-1 rounded-md border border-border bg-background p-2 shadow-dropdown">
                  <span className="px-1 text-sm font-medium text-muted">{m.chat_panel_data_sources()}</span>
                  <LitSourcesList />
                </div>
              )}
            </div>
            <input
              ref={fileInputRef}
              type="file"
              accept="application/pdf,image/png,image/jpeg,image/gif,image/webp"
              multiple
              hidden
              onChange={(e) => {
                addFiles(Array.from(e.target.files ?? []));
                e.target.value = ""; // let the same file be re-picked
              }}
           />
            <IconButton
              type="button"
              className="composer-attach"
              title={m.chat_panel_attach_a_pdf_or_image()}
              aria-label={m.chat_panel_attach_a_pdf_or_image()}
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip size={16} />
            </IconButton>
            {planActive && (
              <Button
                type="button"
                variant="ghost"
                active
                className="group"
                title={m.chat_panel_exit_plan_mode()}
                aria-label={m.chat_panel_exit_plan_mode()}
                onClick={() => void exitPlanMode()}
              >
                <span className="relative size-4" aria-hidden="true">
                  <Lightbulb className="absolute inset-0 transition-opacity group-hover:opacity-0 group-focus-visible:opacity-0" size={16} strokeWidth={1.6} />
                  <X className="absolute inset-0 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" size={16} strokeWidth={1.8} />
                </span>
                <span>{m.chat_panel_plan()}</span>
              </Button>
            )}
            <div className="min-w-0 flex-1" />
            {/* The model picker reflects the open session (harness locked once it
                exists); the global default only applies before the first
                message. */}
            <div className="flex min-w-0 items-center">
              <ModelPicker
                value={composerSelection}
                onSelect={selectModel}
                permissionChoices={activeHarness?.agentReady ? (opts?.permissionModes ?? []) : []}
                defaultPermissionId={opts?.defaultPermissionMode ?? null}
                onSelectPermission={setPermissionMode}
                reasoningChoices={activeHarness?.agentReady ? reasoning.choices : []}
                defaultReasoningId={reasoning.defaultId}
                onSelectReasoning={setReasoningLevel}
                onHarnesses={setHarnesses}
                lockHarness={!!openSession}
             />
              <ContextMeter usage={openSession?.contextUsage} />
            </div>
            {busy && !pendingQuestion ? (
              // Stop whenever the turn is busy and typed text has nowhere to
              // go — actively streaming, or held on a plan/permission card
              // (their cards are the affordance; send() can't service them).
              // Send stays only when it actually works: idle, or a held
              // QUESTION card that owns typed text.
              <IconButton className="send-btn" variant="stop" title={m.chat_panel_stop()} aria-label={m.chat_panel_stop()} onClick={stop}>
                <X size={16} />
              </IconButton>
            ) : (
              <IconButton
                className="send-btn"
                variant="primary"
                title={m.chat_panel_send()}
                aria-label={m.chat_panel_send()}
                onClick={() => void send()}
                disabled={
                  !activeHarness?.agentReady ||
                  (!draft.trim() && attachments.length === 0 && annotations.length === 0)
                }
              >
                <CornerDownLeft size={16} />
              </IconButton>
            )}
          </div>
        </div>
      </div>
      </section>
    </>
  );
}
