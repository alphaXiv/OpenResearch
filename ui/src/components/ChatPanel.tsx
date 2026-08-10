import {
  ArrowUpRight,
  BookOpen,
  ChartSpline,
  Check,
  ChevronRight,
  CornerDownLeft,
  FileText,
  FlaskConical,
  FolderOpen,
  GitBranch,
  HelpCircle,
  MoreHorizontal,
  PanelLeft,
  Paperclip,
  Package,
  Plus,
  SlidersHorizontal,
  Users,
  X,
} from "lucide-react";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { BrandMark } from "./Wordmark";
import {
  chatAttachmentUrl,
  createChatSession,
  deleteChatSession,
  DEMO_FIGURE_SESSION_ID,
  DEMO_LITERATURE_SESSION_ID,
  DEMO_MAIN_SESSION_ID,
  DEMO_PROJECT_ID,
  getChatMessages,
  getSkills,
  interruptChat,
  listChatSessions,
  reasoningFor,
  reconcileReasoning,
  renameChatSession,
  respondChat,
  sendChatMessage,
  setChatSessionArchived,
  type ChatImageAttachment,
  type ChatMessage,
  type ChatPart,
  type ChatPrompt,
  type ChatSession,
  type Harness,
  type PromptAnswer,
  type SkillInfo,
} from "../api";
import { onChatEvent } from "../events";
import { LitSourceLogo, parseOrxLit, paperUrl } from "./LitSourceLogo";
import { LitSourcesPicker } from "./LitSourcesPicker";
import { Md } from "./Md";
import { PlanStrip } from "./PlanStrip";
import { SETTINGS_NAV, type SettingsTab } from "./SettingsPage";
import { SkillMenu } from "./SkillMenu";
import {
  defaultSelection,
  HARNESS_LABELS,
  ModelPicker,
  OptionPicker,
  usePopover,
  type ModelSelection,
} from "./ModelPicker";
import { ContextMeter } from "./ContextMeter";
import { renderNote } from "./agentNote";
import { loadReadDemoSessions, markDemoSessionRead } from "../demoSessionState";
import { ICON_BUTTON_BASE_CLASS_NAME, ICON_BUTTON_CLASS_NAME, MODEL_ITEM_CLASS_NAME, PAPER_TITLE_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

const TOOL_LINE_CLASS_NAME = [
  "tool-line flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap",
  "text-md text-subtext",
].join(" ");

const PROMPT_COLLAPSED_CLASS_NAME = [
  "prompt-collapsed text-muted text-md my-1 mx-0 [&_summary]:flex",
  "[&_summary]:items-baseline [&_summary]:gap-2 [&_summary]:cursor-pointer",
  "[&_summary]:list-none [&_summary]:select-none [&_summary::-webkit-details-marker]:hidden",
  "[&_summary::after]:content-['›'] [&_summary::after]:text-muted",
  "[&_summary::after]:transition-transform [&_summary::after]:duration-80 [&_summary::after]:ease-standard [&[open]_summary::after]:rotate-90",
].join(" ");

const PROMPT_COLLAPSED_BODY_CLASS_NAME = [
  "prompt-collapsed-body mt-1.5 pl-3 border-l-2 border-l-border",
  "text-md text-subtext",
].join(" ");

const PROMPT_HEAD_CLASS_NAME = [
  "prompt-head text-xs font-semibold text-text",
  "[&_code]:font-mono [&_code]:text-sm [&_code]:text-text",
].join(" ");

const PROMPT_ACTIONS_CLASS_NAME = [
  "prompt-actions flex flex-wrap gap-2 [&_.btn-primary]:inline-flex",
  "[&_.btn-primary]:items-center [&_.btn-primary]:gap-1.5 [&_.btn-primary]:py-1.5 [&_.btn-primary]:px-[13px]",
  "[&_.btn-primary]:font-[inherit] [&_.btn-primary]:text-sm",
  "[&_.btn-primary]:font-semibold [&_.btn-primary]:border [&_.btn-primary]:border-transparent",
  "[&_.btn-primary]:rounded-sm [&_.btn-primary]:cursor-pointer",
  "[&_.btn-primary]:transition-[background,border-color] [&_.btn-primary]:duration-80 [&_.btn-primary]:ease-standard [&_.btn-ghost]:inline-flex",
  "[&_.btn-ghost]:items-center [&_.btn-ghost]:gap-1.5 [&_.btn-ghost]:py-1.5 [&_.btn-ghost]:px-[13px]",
  "[&_.btn-ghost]:font-[inherit] [&_.btn-ghost]:text-sm",
  "[&_.btn-ghost]:font-semibold [&_.btn-ghost]:border [&_.btn-ghost]:border-transparent",
  "[&_.btn-ghost]:rounded-sm [&_.btn-ghost]:cursor-pointer",
  "[&_.btn-ghost]:transition-[background,border-color] [&_.btn-ghost]:duration-80 [&_.btn-ghost]:ease-standard",
  "[&_.btn-primary]:bg-primary [&_.btn-primary]:text-background",
  "[&_.btn-primary:hover:not(:disabled)]:opacity-90 [&_.btn-ghost]:bg-transparent",
  "[&_.btn-ghost]:border-border [&_.btn-ghost]:text-subtext",
  "[&_.btn-ghost:hover:not(:disabled)]:border-border-strong",
  "[&_.btn-ghost:hover:not(:disabled)]:text-text",
  "[&_.btn-ghost:hover:not(:disabled)]:bg-surface [&_button:disabled]:opacity-50",
  "[&_button:disabled]:cursor-default",
].join(" ");

// --- chat state --------------------------------------------------------------

interface ChatState {
  messagesBySession: Record<string, ChatMessage[]>;
  busySessions: Set<string>;
}

type Action =
  | { type: "reset" }
  | { type: "seed"; sessionId: string; messages: ChatMessage[]; onlyIfAbsent?: boolean }
  | { type: "upsertMessage"; sessionId: string; message: ChatMessage }
  | {
      type: "optimisticUser";
      sessionId: string;
      text: string;
      attachments: { url: string; mediaType: string; name?: string }[];
    }
  | { type: "busy"; sessionId: string; busy: boolean }
  // `known` scopes the reseed: flags for sessions outside it (other projects —
  // busy events aren't project-filtered) are carried forward, not wiped.
  | { type: "seedBusy"; sessions: string[]; known: string[] }
  | { type: "forget"; sessionId: string };

const LOCAL_PREFIX = "local-";

function upsertMessage(list: ChatMessage[], message: ChatMessage): ChatMessage[] {
  const i = list.findIndex((m) => m.id === message.id);
  if (i >= 0) {
    const next = list.slice();
    next[i] = message;
    return next;
  }
  // The server's copy of the user message replaces the optimistic local one.
  const cleaned =
    message.role === "user" ? list.filter((m) => !m.id.startsWith(LOCAL_PREFIX)) : list;
  return [...cleaned, message];
}

function reducer(state: ChatState, action: Action): ChatState {
  switch (action.type) {
    case "reset":
      return { messagesBySession: {}, busySessions: new Set() };
    case "seed":
      // onlyIfAbsent: recover a failed fetch without clobbering messages that
      // streamed in via SSE during it (a `message` event already created the key).
      if (action.onlyIfAbsent && action.sessionId in state.messagesBySession) return state;
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: action.messages },
      };
    case "upsertMessage": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      return {
        ...state,
        messagesBySession: {
          ...state.messagesBySession,
          [action.sessionId]: upsertMessage(list, action.message),
        },
      };
    }
    case "optimisticUser": {
      const list = state.messagesBySession[action.sessionId] ?? [];
      const parts: ChatPart[] = action.text
        ? [{ id: "p0", type: "text", text: action.text }]
        : [];
      // Data URLs stand in until the server's copy arrives with file names.
      action.attachments.forEach((a, i) =>
        parts.push({ id: `img${i}`, type: "image", text: a.url, name: a.name }),
      );
      const msg: ChatMessage = {
        id: `${LOCAL_PREFIX}${Date.now()}`,
        role: "user",
        parts,
        createdAt: Date.now(),
      };
      return {
        ...state,
        messagesBySession: { ...state.messagesBySession, [action.sessionId]: [...list, msg] },
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
    case "forget": {
      // Deleted session: drop its transcript and busy flag so a same-id event
      // arriving late can't render stale state.
      const messagesBySession = { ...state.messagesBySession };
      delete messagesBySession[action.sessionId];
      const busySessions = new Set(state.busySessions);
      busySessions.delete(action.sessionId);
      return { messagesBySession, busySessions };
    }
  }
}

// --- rendering ---------------------------------------------------------------

function toolStatusClass(status: string | undefined): string {
  const base = "tool-status w-1.5 h-1.5 rounded-full shrink-0";
  if (status === "error") return `${base} error bg-accent-red`;
  if (status === "completed") return `${base} bg-muted`;
  return `${base} running bg-accent-amber animate-[or-pulse_1.2s_ease-in-out_infinite]`;
}

function relTime(ts: number | undefined): string {
  if (!ts) return "";
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return "now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

/** The last path segment, for compact display ("src/a/b.rs" → "b.rs"). */
function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

/** Claude-desktop-style one-liner: a verb + target, e.g. "Read hello.py",
 * "Ran echo hello". Falls back to the raw tool name. */
function toolLine(part: ChatPart): string {
  const tool = part.tool ?? "tool";
  const input = part.state?.input ?? {};
  const cmd = typeof input.command === "string" ? input.command : null;
  const fp = typeof input.filePath === "string" ? input.filePath : null;
  const desc = typeof input.description === "string" ? input.description : null;
  switch (tool) {
    case "Bash":
    case "bash":
      return cmd ? `Ran ${cmd}` : "Ran command";
    case "Read":
      return fp ? `Read ${baseName(fp)}` : "Read file";
    case "Edit":
    case "Write":
    case "NotebookEdit":
      return fp ? `Edited ${baseName(fp)}` : "Edited file";
    case "Grep":
      return typeof input.pattern === "string" ? `Searched “${input.pattern}”` : "Searched";
    case "Glob":
      return typeof input.pattern === "string" ? `Found ${input.pattern}` : "Listed files";
    case "WebSearch": {
      // Codex web-tool actions: search {query}, openPage {url},
      // findInPage {pattern, url} — query is empty for the latter two.
      if (typeof input.query === "string" && input.query)
        return `Searched the web: “${input.query}”`;
      const url = typeof input.url === "string" ? input.url : null;
      if (typeof input.pattern === "string" && input.pattern && url)
        return `Searched “${input.pattern}” in ${url}`;
      if (url) return `Opened ${url}`;
      // codex reports page visits as an opaque {type:"other"} action —
      // "searched" would be wrong, all we know is the web tool ran.
      if (input.type === "other") return "Browsed the web";
      return desc ?? "Searched the web";
    }
    case "WebFetch":
      if (typeof input.url === "string") return `Fetched ${input.url}`;
      return desc ?? "Fetched a page";
    case "Task":
      return desc ?? "Ran a subagent";
    case "subagent":
      return subagentLine(input);
    case "error":
      return "Error";
    case "interrupted":
      return "Interrupted";
    default: {
      const detail = desc ?? fp ?? cmd ?? part.state?.title ?? "";
      return detail ? `${tool}: ${detail}` : tool;
    }
  }
}

/** Richer summary for the tool row: `orx lit`/`orx paper` Bash calls render as a
 * real search (source logo + natural language) instead of raw shell output.
 * Everything else falls back to the plain `toolLine` string. */
function toolSummary(part: ChatPart): React.ReactNode {
  if (part.tool === "Bash" || part.tool === "bash") {
    const cmd = part.state?.input?.command;
    const call = typeof cmd === "string" ? parseOrxLit(cmd) : null;
    if (call) {
      // The logo already names the source, so the text doesn't repeat it.
      const text =
        call.kind === "lit"
          ? call.query
            ? `Searching for “${call.query}”`
            : "Searching"
          : call.id
            ? `Reading ${call.id}`
            : "Reading a paper";
      // A fetched paper links out to its page on the source (with an external-link
      // affordance so it reads as clickable); the search rows are plain text. The
      // anchor is the click's activation target, so it navigates without toggling
      // the row open — stopPropagation just guards against any future row handler.
      const body =
        call.kind === "paper" && call.id ? (
          <a
            className="tool-lit-link inline-flex items-center gap-1 min-w-0 text-inherit no-underline [&:hover]:text-primary [&:hover_.tool-lit-text]:underline [&:hover_.tool-lit-ext]:opacity-100"
            href={paperUrl(call.source, call.id)}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(e) => e.stopPropagation()}
          >
            <span className="tool-lit-text min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{text}</span>
            <ArrowUpRight className="tool-lit-ext flex-none opacity-50" size={13} aria-hidden="true" />
          </a>
        ) : (
          <span className="tool-lit-text min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{text}</span>
        );
      return (
        <span className="tool-lit inline-flex items-center gap-2 min-w-0 max-w-full align-bottom">
          <LitSourceLogo source={call.source} />
          {body}
        </span>
      );
    }
  }
  return toolLine(part);
}

/** Readable one-liner for a Codex sub-agent spawn/activity row, from the
 * collab item fields the backend put in `state.input`. */
function subagentLine(input: Record<string, unknown>): string {
  const trim = (s: string) => (s.length > 60 ? `${s.slice(0, 60)}…` : s);
  const prompt = typeof input.prompt === "string" && input.prompt ? ` — “${trim(input.prompt)}”` : "";
  // collabAgentToolCall carries `tool`; subAgentActivity carries `kind`.
  switch (typeof input.tool === "string" ? input.tool : "") {
    case "spawnAgent":
      return `Spawned agent${prompt}`;
    case "sendInput":
      return `Sent input to agent${prompt}`;
    case "resumeAgent":
      return "Resumed agent";
    case "wait":
      return "Waiting on agent";
    case "closeAgent":
      return "Closed agent";
  }
  switch (typeof input.kind === "string" ? input.kind : "") {
    case "started":
      return "Sub-agent started";
    case "interacted":
      return "Sub-agent activity";
    case "interrupted":
      return "Sub-agent interrupted";
  }
  return "Sub-agent";
}

/** One expandable tool row inside a group: gray summary line, click to reveal
 * the input + output. */
function ToolRow({ part, onOpenFile }: { part: ChatPart; onOpenFile?: (path: string) => void }) {
  const state = part.state;
  const output = state?.error || state?.output || "";
  const cmd = typeof state?.input?.command === "string" ? state.input.command : null;
  const filePath = typeof state?.input?.filePath === "string" ? state.input.filePath : null;
  const hasDetail = Boolean(output || cmd || filePath);
  return (
    <details className="tool-row flex flex-col [&_summary]:flex [&_summary]:items-center [&_summary]:gap-2 [&_summary]:py-[3px] [&_summary]:px-1 [&_summary]:cursor-pointer [&_summary]:list-none [&_summary]:select-none [&_summary]:min-w-0 [&_summary]:rounded-sm [&_summary:hover]:bg-surface [&_summary::-webkit-details-marker]:hidden" open={false}>
      <summary>
        <span className={toolStatusClass(state?.status)} />
        <span className={TOOL_LINE_CLASS_NAME}>{toolSummary(part)}</span>
        {filePath && onOpenFile && (
          <button
            className="tool-open shrink-0 text-xs text-primary [&:hover]:underline file-link"
            title={`Open ${filePath}`}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onOpenFile(filePath);
            }}
          >
            open
          </button>
        )}
      </summary>
      {hasDetail && (
        <div className="tool-detail mt-0.5 mr-0 mb-1 ml-3.5">
          {cmd && <div className="tool-cmd-full py-1.5 px-2.5 font-mono text-xs text-text bg-surface rounded-sm whitespace-pre-wrap wrap-anywhere">{cmd}</div>}
          {/* Safety net for pre-cap stored transcripts; the backend caps live
              tool output at 16k (TOOL_TEXT_CAP), so this slice must stay
              above that or it clips the truncation marker. */}
          {output && <div className="tool-output mt-[3px] py-1.5 px-2.5 font-mono text-xs text-subtext whitespace-pre-wrap wrap-anywhere max-h-65 overflow-y-auto bg-background border border-border-variant rounded-sm">{output.slice(0, 20000)}</div>}
        </div>
      )}
    </details>
  );
}

/** A run of consecutive tool calls. A single call renders as its own row
 * (click reveals input/output); several collapse behind one "Used N tools"
 * line that expands to every row, auto-expanded while one is still running. */
function ToolGroup({ parts, onOpenFile }: { parts: ChatPart[]; onOpenFile?: (path: string) => void }) {
  const running = parts.some((p) => p.state?.status === "running");
  const errored = parts.some((p) => p.state?.status === "error");
  const [open, setOpen] = useState(false);

  // A single tool needs no group wrapper: its ToolRow already shows the same
  // line (dot + toolLine) and expands to the input/output directly. The
  // summary-plus-rows shape would paint the identical line twice. A running
  // lone row stays collapsed by design — the old auto-expand only revealed a
  // duplicate line whose detail was collapsed anyway.
  if (parts.length === 1) {
    return (
      <div className={`tool-group my-0.5 mx-0 [&.has-error_.tool-group-summary]:text-accent-red [&.has-error_>_.tool-row_.tool-line]:text-accent-red ${errored ? "has-error" : ""}`}>
        <ToolRow part={parts[0]} onOpenFile={onOpenFile} />
      </div>
    );
  }

  // While a tool is in flight, show the rows live; collapse once the run
  // settles. Because a running group is always expanded, a summary echoing
  // the running tool's line would sit directly above the identical row — the
  // count never duplicates.
  const expanded = open || running;
  const summary = `Used ${parts.length} tools`;

  return (
    <div className={`tool-group my-0.5 mx-0 [&.has-error_.tool-group-summary]:text-accent-red [&.has-error_>_.tool-row_.tool-line]:text-accent-red ${errored ? "has-error" : ""}`}>
      <button className="tool-group-summary flex items-center gap-2 w-full py-[3px] px-0.5 cursor-pointer text-muted text-md text-left rounded-sm [&:hover]:text-subtext [&:hover]:bg-surface" onClick={() => setOpen((v) => !v)}>
        <span className={toolStatusClass(running ? "running" : errored ? "error" : "completed")} />
        <span className={TOOL_LINE_CLASS_NAME}>{summary}</span>
        <ChevronRight size={12} className={`tool-chevron shrink-0 text-muted transition-transform duration-120 ease-standard [&.open]:rotate-90 ${expanded ? "open" : ""}`} />
      </button>
      {expanded && (
        <div className="tool-group-rows flex flex-col gap-px mt-0.5 mr-0 mb-1 ml-[7px] pl-2.5 border-l border-l-border-variant [&_.tool-status]:hidden">
          {parts.map((p) => (
            <ToolRow key={p.id} part={p} onOpenFile={onOpenFile} />
          ))}
        </div>
      )}
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
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
  onOpenPlan?: (plan: string, promptId: string) => void;
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
      // No echo (`approved` absent — stale-card cleanup, pre-echo history):
      // neutral "Resolved", not a checkmark implying approval. A denial with
      // a note asked for changes; without one it was a plain rejection.
      const outcome =
        p.approved === true
          ? "Plan approved"
          : p.approved === false
            ? p.note
              ? "Revision requested"
              : "Rejected"
            : "Resolved";
      const outcomeClass =
        p.approved === true
          ? "approved"
          : p.approved === false
            ? p.note
              ? "revised"
              : "rejected"
            : "";
      return (
        <details className={PROMPT_COLLAPSED_CLASS_NAME}>
          <summary>
            <span className="prompt-collapsed-title font-semibold wrap-anywhere">
              {p.synthesized ? "Plan" : "Proposed plan"}
            </span>
            <span className={`prompt-outcome text-sm text-subtext wrap-anywhere [&.approved]:text-accent-green [&.chosen]:text-accent-green [&.approved::before]:content-['✓_'] [&.chosen::before]:content-['✓_'] [&.revised]:text-accent-amber [&.rejected]:text-accent-amber ${outcomeClass}`}>{outcome}</span>
          </summary>
          <div className={PROMPT_COLLAPSED_BODY_CLASS_NAME}>
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
    return (
      <details className={PROMPT_COLLAPSED_CLASS_NAME}>
        <summary>
          <span className="prompt-collapsed-title font-semibold wrap-anywhere">{p.header || p.question || "Question"}</span>
          <span className={`prompt-outcome text-sm text-subtext wrap-anywhere [&.approved]:text-accent-green [&.chosen]:text-accent-green [&.approved::before]:content-['✓_'] [&.chosen::before]:content-['✓_'] [&.revised]:text-accent-amber [&.rejected]:text-accent-amber ${chosen ? "chosen" : ""}`}>{chosen || "Resolved"}</span>
        </summary>
        <div className={PROMPT_COLLAPSED_BODY_CLASS_NAME}>
          {/* The summary title already shows the question when there's no header. */}
          {p.header && p.question && <div className="prompt-q text-base font-semibold leading-normal text-text">{p.question}</div>}
          {(p.options ?? []).length > 0 && (
            <ul className="prompt-collapsed-options mt-1.5 mx-0 mb-0 pl-4.5 [&_.sel]:text-text [&_.sel]:font-semibold">
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
      <div className={`prompt-card my-2 mx-0 py-3 px-3.5 border border-border border-l-[3px] border-l-border rounded-sm bg-surface flex flex-col gap-[9px] [&.plan]:border-l-accent-blue [&.permission]:border-l-accent-amber [&.question]:border-l-accent-purple [&.readonly]:opacity-60 plan ${done ? "readonly" : ""}`}>
        <div className={PROMPT_HEAD_CLASS_NAME}>
          {p.synthesized ? "Plan mode — ready to proceed?" : "Proposed plan"}
        </div>
        <div className={`prompt-plan text-base leading-[1.6] text-text max-h-85 overflow-y-auto [&.clamped]:max-h-[9.5em] [&.clamped]:overflow-hidden [&.clamped]:relative [&.clamped::after]:content-[''] [&.clamped::after]:absolute [&.clamped::after]:inset-x-0 [&.clamped::after]:bottom-0 [&.clamped::after]:top-auto [&.clamped::after]:h-8.5 [&.clamped::after]:bg-[linear-gradient(to_bottom,_transparent,_var(--surface))] [&.clamped::after]:pointer-events-none ${docked ? "clamped" : ""}`}>
          <Md text={p.plan ?? ""} onOpenFile={onOpenFile} />
        </div>
        {docked && (
          <button className="prompt-plan-open self-start border-0 bg-transparent text-accent-blue text-sm p-0 cursor-pointer [&:hover]:underline" onClick={() => onOpenPlan(p.plan ?? "", part.id)}>
            View full plan
          </button>
        )}
        {/* Strip-less fallback (unreachable in the main app — App always
            provides onOpenPlan): same action semantics as the strip. */}
        {!done && !docked && (
          <div className={PROMPT_ACTIONS_CLASS_NAME}>
            <button className="btn-primary" onClick={() => respond({ approve: true, resumeMode: "auto" })}>
              Accept and auto mode
            </button>
            <button className="btn-ghost" onClick={() => respond({ approve: true, resumeMode: "bypass" })}>
              Accept and bypass all
            </button>
            <button className="btn-ghost" onClick={() => respond({ approve: false })}>
              Reject
            </button>
          </div>
        )}
      </div>
    );
  }

  if (p.kind === "permission") {
    const summary =
      (typeof p.toolInput?.command === "string" && p.toolInput.command) ||
      (typeof p.toolInput?.filePath === "string" && p.toolInput.filePath) ||
      "";
    // Codex approval cards ship a human-readable reason (and fileChange cards
    // carry nothing else) — show it so the user knows what they're granting.
    const reason =
      (typeof p.toolInput?.reason === "string" && p.toolInput.reason) || "";
    return (
      <div className={`prompt-card my-2 mx-0 py-3 px-3.5 border border-border border-l-[3px] border-l-border rounded-sm bg-surface flex flex-col gap-[9px] [&.plan]:border-l-accent-blue [&.permission]:border-l-accent-amber [&.question]:border-l-accent-purple [&.readonly]:opacity-60 permission ${done ? "readonly" : ""}`}>
        <div className={PROMPT_HEAD_CLASS_NAME}>
          Permission needed: <code>{p.tool}</code>
        </div>
        {summary && <div className="prompt-sub text-sm text-subtext wrap-anywhere">{summary}</div>}
        {reason && <div className="prompt-sub text-sm text-subtext wrap-anywhere">{reason}</div>}
        {!done && (
          // No resumeMode: the harness picks the right one for an approval.
          // Claude resumes under `bypass` (the only mode that actually grants a
          // blocked tool — acceptEdits would re-deny Bash); inline harnesses
          // (opencode) reply once/reject keyed off `approve`. Deny denies either way.
          <div className={PROMPT_ACTIONS_CLASS_NAME}>
            <button className="btn-primary" onClick={() => respond({ approve: true })}>
              Allow
            </button>
            <button className="btn-ghost" onClick={() => respond({ approve: false })}>
              Deny
            </button>
          </div>
        )}
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
    <div className={`prompt-card my-2 mx-0 py-3 px-3.5 border border-border border-l-[3px] border-l-border rounded-sm bg-surface flex flex-col gap-[9px] [&.plan]:border-l-accent-blue [&.permission]:border-l-accent-amber [&.question]:border-l-accent-purple [&.readonly]:opacity-60 question ${done ? "readonly" : ""}`}>
      {p.header && <div className={PROMPT_HEAD_CLASS_NAME}>{p.header}</div>}
      {p.question && <div className="prompt-q text-base font-semibold leading-normal text-text">{p.question}</div>}
      <div className="prompt-options flex flex-col gap-1.5">
        {(p.options ?? []).map((o) => {
          const sel = picked.includes(o.label);
          return (
            <button
              key={o.label}
              className={`prompt-option flex flex-col items-start gap-0.5 w-full py-2 px-[11px] text-left border border-border rounded-sm bg-background text-text cursor-pointer transition-[border-color,background] duration-80 ease-standard [&:hover:not(:disabled)]:border-border-strong [&:hover:not(:disabled)]:bg-surface [&.sel]:border-primary [&.sel]:bg-primary-subtle [&:disabled]:cursor-default ${sel ? "sel" : ""}`}
              disabled={done}
              onClick={() => (done ? undefined : p.multiSelect ? toggle(o.label) : respond({ answers: [o.label] }))}
            >
              <span className="prompt-option-label block text-md font-semibold">{o.label}</span>
              {o.description && <span className="prompt-option-desc block text-sm font-normal leading-[1.45] text-subtext">{o.description}</span>}
            </button>
          );
        })}
      </div>
      {p.multiSelect && !done && (
        <div className={PROMPT_ACTIONS_CLASS_NAME}>
          <button
            className="btn-primary"
            disabled={picked.length === 0}
            onClick={() => respond({ answers: picked })}
          >
            Submit
          </button>
        </div>
      )}
    </div>
  );
}

/** Whether a part paints anything in the transcript. The single source of
 * truth for "invisible": empty text/reasoning (encrypted-thinking models
 * stored these before the harness-side skip existed) and resolved permission
 * cards (which leave no trace). Shared by `messageHasVisibleContent` and
 * `renderParts` so the two can't drift. */
function partIsVisible(part: ChatPart): boolean {
  if (part.type === "prompt")
    return !!part.prompt && !(part.prompt.resolved && part.prompt.kind === "permission");
  if (part.type === "text" || part.type === "reasoning") return !!part.text;
  return true; // tool, image, …
}

/** Whether a message renders anything once resolved-permission cards vanish —
 * a bridge permission card rides its own message, so resolving it leaves the
 * message empty and it must drop out of the transcript entirely. */
function messageHasVisibleContent(m: ChatMessage): boolean {
  if (m.role === "user") return true;
  return m.parts.some(partIsVisible);
}

/** Memoized: streaming re-broadcasts the whole updated message ~7x/sec, and
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

const Message = memo(function Message({
  message,
  onOpenFile,
  onOpenRun,
  onRespond,
  onOpenPlan,
  onOpenSubagent,
  skills,
}: {
  message: ChatMessage;
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
  onOpenRun?: (runId: string) => void;
  onRespond?: (answer: PromptAnswer) => void;
  /** Open a plan's full markdown in the right pane (plan cards/strip). */
  onOpenPlan?: (plan: string, promptId: string) => void;
  /** Open a sub-agent's transcript in the right pane (spawn-row "view"). */
  onOpenSubagent?: (spawnPartId: string) => void;
  /** Known slash-skills, for rendering a leading `/name` as a command chip. */
  skills?: SkillInfo[];
}) {
  if (message.role === "user") {
    const text = message.parts
      .filter((p) => p.type === "text")
      .map((p) => p.text ?? "")
      .join("\n");
    // A leading known `/command` renders as the same chip the composer shows.
    // Unknown commands (or skills removed since) fall back to plain text.
    const slash = text.match(/^\/(\S+)([\s\S]*)$/);
    const command = slash ? skills?.find((s) => s.name === slash[1]) : undefined;
    // Optimistic parts carry a data URL; server parts carry a file name.
    const attachments = message.parts
      .filter((p) => p.type === "image" && p.text)
      .map(attachmentPartView);
    const images = attachments.filter((a) => !a.isPdf);
    const files = attachments.filter((a) => a.isPdf);
    return (
      <div className="msg-user self-end max-w-[88%] bg-surface rounded-[16px] py-2.5 px-[15px] text-base whitespace-pre-wrap wrap-anywhere [&_.skill-chip]:mr-0.5 [&_.skill-chip]:align-baseline">
        {command ? (
          <>
            <span className="skill-chip inline-flex items-center py-px px-[7px] font-mono text-md font-medium text-primary bg-primary-subtle border border-border-variant rounded-sm">/{command.name}</span>
            {slash![2]}
          </>
        ) : (
          text
        )}
        {images.length > 0 && (
          <div className="msg-images flex flex-wrap gap-1.5 mt-2 [&_img]:max-w-55 [&_img]:max-h-40 [&_img]:border [&_img]:border-border-variant [&_img]:rounded-xs [&_img]:block">
            {images.map((a, i) => (
              <a key={i} href={a.src} target="_blank" rel="noreferrer">
                <img src={a.src} alt="attachment" />
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
    );
  }
  return (
    <div className="msg-assistant text-lg leading-[1.62] text-text min-w-0">
      {renderParts(message.parts, { onOpenFile, onOpenRun, onRespond, onOpenPlan, onOpenSubagent })}
    </div>
  );
});

/** Shared assistant-parts renderer, reused for a message body and (recursively)
 * for a sub-agent's nested transcript. Coalesces consecutive tool parts into one
 * collapsed group (Claude-desktop style); text / reasoning / prompt parts break
 * a run and render inline. A sub-agent spawn part (tool `subagent`) also breaks
 * the run and renders as its own nested block. */
function renderParts(
  parts: ChatPart[],
  opts: {
    onOpenFile?: (path: string, line?: number, exp?: string) => void;
    onOpenRun?: (runId: string) => void;
    onRespond?: (answer: PromptAnswer) => void;
    onOpenPlan?: (plan: string, promptId: string) => void;
    onOpenSubagent?: (spawnPartId: string) => void;
  },
): React.ReactNode[] {
  const { onOpenFile, onOpenRun, onRespond, onOpenPlan, onOpenSubagent } = opts;
  const rendered: React.ReactNode[] = [];
  let toolRun: ChatPart[] = [];
  const flushTools = () => {
    if (toolRun.length === 0) return;
    rendered.push(
      <ToolGroup key={`tg-${toolRun[0].id}`} parts={toolRun} onOpenFile={onOpenFile} />,
    );
    toolRun = [];
  };
  for (const part of parts) {
    // A part that renders nothing must not break a tool run either — e.g. the
    // empty reasoning parts encrypted-thinking models produced (stored
    // transcripts predating the ingest-side skip still carry them), or a
    // resolved permission card. Without this, each invisible part splits
    // consecutive tools into single-row groups.
    if (!partIsVisible(part)) continue;
    // A sub-agent spawn part streams its own transcript in `children` — render
    // it as a standalone nested block, not folded into a tool run. The signal is
    // harness-agnostic: Codex tags the row `subagent`, while Claude's `Task` /
    // OpenCode's `task` rows are spawns whenever they carry children.
    if (part.type === "tool" && (part.tool === "subagent" || (part.children?.length ?? 0) > 0)) {
      flushTools();
      rendered.push(
        <SubagentBlock key={part.id} part={part} onOpenSubagent={onOpenSubagent} />,
      );
      continue;
    }
    if (part.type === "tool") {
      toolRun.push(part);
      continue;
    }
    flushTools();
    // The visibility skip above guarantees text/reasoning parts here are
    // non-empty.
    if (part.type === "text")
      rendered.push(
        <Md key={part.id} text={part.text!} onOpenFile={onOpenFile} onOpenRun={onOpenRun} />,
      );
    else if (part.type === "reasoning")
      rendered.push(
        <details key={part.id} className="reasoning text-muted text-md my-0.5 mx-0 [&_summary]:cursor-pointer [&_summary]:list-none [&_summary]:select-none [&_summary]:font-semibold [&[open]]:whitespace-pre-wrap">
          <summary>thinking…</summary>
          {part.text}
        </details>,
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

/** Find a part by id anywhere in a parts tree (depth-first). Used by the
 * right-pane sub-agent tab to locate a spawn part across a session's messages. */
export function findPartById(parts: ChatPart[], id: string): ChatPart | null {
  for (const part of parts) {
    if (part.id === id) return part;
    const nested = part.children && findPartById(part.children, id);
    if (nested) return nested;
  }
  return null;
}

/** The sub-agent's transcript, rendered standalone in the right-pane tab (the
 * only place the transcript is shown — the inline row just opens this). Reuses
 * `renderParts`, so nested sub-agents are themselves click-to-open rows. */
export function SubagentTranscript({
  spawn,
  onOpenFile,
  onOpenRun,
  onOpenSubagent,
}: {
  spawn: ChatPart;
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
  onOpenRun?: (runId: string) => void;
  onOpenSubagent?: (spawnPartId: string) => void;
}) {
  const parts = spawn.children ?? [];
  const running = spawn.state?.status === "running";
  // Gate the empty state on what actually renders, not the raw part count — a
  // stored transcript of nothing but invisible parts must still read as empty.
  const rendered = renderParts(parts, { onOpenFile, onOpenRun, onOpenSubagent });
  return (
    <div className="msg-assistant text-lg leading-[1.62] text-text min-w-0">
      <div className="subagent-tab-header flex items-center gap-2 pb-2 mb-2 border-b border-b-border-variant">
        <span className={toolStatusClass(spawn.state?.status)} />
        <span className={TOOL_LINE_CLASS_NAME}>{toolLine(spawn)}</span>
        {running && <span className="subagent-live shrink-0 text-xs text-accent-amber">live</span>}
      </div>
      {rendered.length === 0 ? (
        <div className="subagent-empty py-[3px] px-1 text-md text-muted">{running ? "Working…" : "No activity"}</div>
      ) : (
        rendered
      )}
    </div>
  );
}

/** A Codex/Claude/OpenCode sub-agent spawn row. A single clickable line — a
 * status dot + label — that opens the sub-agent's full transcript in the
 * right-side panel (like the Claude/Codex desktop apps). The transcript is
 * never expanded inline; the row stays a one-liner whether the sub-agent is
 * running (pulsing dot) or done. */
function SubagentBlock({
  part,
  onOpenSubagent,
}: {
  part: ChatPart;
  onOpenSubagent?: (spawnPartId: string) => void;
}) {
  const errored = part.state?.status === "error";
  return (
    <button
      className={`subagent-row flex items-center gap-2 w-full my-0.5 mx-0 py-[3px] px-1 cursor-pointer text-text text-base text-left rounded-sm [&:hover:not(:disabled)]:bg-surface [&:disabled]:cursor-default [&.has-error]:text-accent-red [&_.tool-line]:text-base [&_.tool-line]:text-text ${errored ? "has-error" : ""}`}
      title="Open sub-agent transcript"
      onClick={() => onOpenSubagent?.(part.id)}
      disabled={!onOpenSubagent}
    >
      <Users size={12} className="subagent-icon shrink-0 text-muted" />
      <span className={toolStatusClass(part.state?.status)} />
      <span className={TOOL_LINE_CLASS_NAME}>{toolLine(part)}</span>
      <ChevronRight size={12} className="subagent-row-chevron shrink-0 text-muted" />
    </button>
  );
}

/** Memoized transcript: composer keystrokes re-render ChatPanel (draft state
 * lives there), and this boundary keeps them from re-allocating N Message
 * elements and running N memo comparisons. Every prop passed here must stay
 * referentially stable across keystrokes (memoized/useCallback, never inline)
 * or the boundary silently breaks — with that held, typing costs one shallow
 * compare instead of O(messages) work. */
const Transcript = memo(function Transcript({
  messages,
  onOpenFile,
  onOpenRun,
  onRespond,
  onOpenPlan,
  onOpenSubagent,
  skills,
}: {
  messages: ChatMessage[];
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
  onOpenRun?: (runId: string) => void;
  onRespond?: (answer: PromptAnswer) => void;
  onOpenPlan?: (plan: string, promptId: string) => void;
  onOpenSubagent?: (spawnPartId: string) => void;
  skills?: SkillInfo[];
}) {
  return (
    <>
      {messages.filter(messageHasVisibleContent).map((m) => (
        <Message
          key={m.id}
          message={m}
          onOpenFile={onOpenFile}
          onOpenRun={onOpenRun}
          onRespond={onRespond}
          onOpenPlan={onOpenPlan}
          onOpenSubagent={onOpenSubagent}
          skills={skills}
        />
      ))}
    </>
  );
});

// --- session rail ------------------------------------------------------------

type SessionFilter = "active" | "archived" | "all";

/** Whether the rail's current filter shows a session in this archived state. */
const matchesFilter = (filter: SessionFilter, archived: boolean) =>
  filter === "all" ? true : filter === "archived" ? archived : !archived;

/** Menu label + rail section heading per filter — "Recents" for the default view. */
const SESSION_FILTERS: { id: SessionFilter; label: string; railLabel: string }[] = [
  { id: "active", label: "Active", railLabel: "Recents" },
  { id: "archived", label: "Archived", railLabel: "Archived" },
  { id: "all", label: "All", railLabel: "All sessions" },
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
      <button
        className={`${ICON_BUTTON_BASE_CLASS_NAME} rail-filter-btn w-6 h-6 rounded-sm ${value !== "active" ? "active" : ""}`}
        title="Filter sessions"
        aria-label="Filter sessions"
        onClick={() => setOpen((v) => !v)}
      >
        <SlidersHorizontal size={13} />
      </button>
      {open && (
        <div className="option-menu absolute bottom-[calc(100%_+_8px)] left-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:left-auto [&.align-right]:right-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:left-auto [&.session-menu]:right-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down align-right">
          {SESSION_FILTERS.map((f) => (
            <button
              key={f.id}
              className={MODEL_ITEM_CLASS_NAME}
              onClick={() => {
                onChange(f.id);
                setOpen(false);
              }}
            >
              <span>{f.label}</span>
              {value === f.id && <Check size={13} />}
            </button>
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
      className={`session-row relative flex items-center gap-2 w-full text-left py-[7px] px-2.5 rounded-md text-md text-text cursor-pointer select-none [&:hover]:bg-surface [&.active]:bg-surface [&.active]:font-medium [&_.session-dot]:w-3.5 [&_.session-dot]:inline-flex [&_.session-dot]:items-center [&_.session-dot]:justify-center [&_.session-dot]:shrink-0 [&_.session-title]:flex-1 [&_.session-title]:min-w-0 [&_.session-title]:overflow-hidden [&_.session-title]:text-ellipsis [&_.session-title]:whitespace-nowrap [&.unread_.session-title]:font-semibold [&_.session-time]:text-2xs [&_.session-time]:text-muted [&_.session-time]:shrink-0 [&_.session-menu-btn]:hidden [&_.session-menu-btn]:items-center [&_.session-menu-btn]:justify-center [&_.session-menu-btn]:w-4 [&_.session-menu-btn]:h-4 [&_.session-menu-btn]:-my-0.5 [&_.session-menu-btn]:mx-0 [&_.session-menu-btn]:rounded-sm [&_.session-menu-btn]:text-muted [&_.session-menu-btn]:shrink-0 [&_.session-menu-btn:hover]:text-text [&_.session-menu-btn:hover]:bg-panel [&:hover_.session-menu-btn]:inline-flex [&:focus-within_.session-menu-btn]:inline-flex [&.menu-open_.session-menu-btn]:inline-flex [&:hover_.session-time]:hidden [&:focus-within_.session-time]:hidden [&.menu-open_.session-time]:hidden [&_.busy-dot]:w-[7px] [&_.busy-dot]:h-[7px] [&_.busy-dot]:rounded-full [&_.busy-dot]:bg-primary [&_.busy-dot]:animate-[or-pulse_1.2s_infinite] [&_.busy-dot]:shrink-0 [&_.unread-dot]:w-[7px] [&_.unread-dot]:h-[7px] [&_.unread-dot]:rounded-full [&_.unread-dot]:bg-primary [&_.unread-dot]:shrink-0 [&_.busy-dot.waiting]:animate-none [&_.session-title-input]:flex-1 [&_.session-title-input]:min-w-0 [&_.session-title-input]:py-px [&_.session-title-input]:px-[5px] [&_.session-title-input]:-my-0.5 [&_.session-title-input]:mx-0 [&_.session-title-input]:[font:inherit] [&_.session-title-input]:text-text [&_.session-title-input]:bg-background [&_.session-title-input]:border [&_.session-title-input]:border-primary [&_.session-title-input]:rounded-sm [&_.session-title-input]:outline-none [&.editing]:bg-surface [&.editing]:cursor-default [&.editing_.session-menu-btn]:hidden [&.editing_.session-time]:hidden ${active ? "active" : ""}  ${unread ? "unread" : ""}  ${open ? "menu-open" : ""}  ${
        editing ? "editing" : ""
      }`}
      title={`${HARNESS_LABELS[session.harness]}${session.model ? ` · ${session.model}` : ""}`}
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
      {editing ? (
        <input
          ref={inputRef}
          className="session-title-input"
          aria-label="Session title"
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
        title="Session options"
        aria-label="Session options"
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        <MoreHorizontal size={14} />
      </button>
      {open && (
        <div className="option-menu absolute bottom-[calc(100%_+_8px)] left-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:left-auto [&.align-right]:right-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:left-auto [&.session-menu]:right-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down session-menu">
          <button
            className={MODEL_ITEM_CLASS_NAME}
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              startEditing();
            }}
          >
            <span>Rename</span>
          </button>
          <button
            className={MODEL_ITEM_CLASS_NAME}
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onSetArchived(!session.archived);
            }}
          >
            <span>{session.archived ? "Unarchive" : "Archive"}</span>
          </button>
          <button
            className={`${MODEL_ITEM_CLASS_NAME} danger`}
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onDelete();
            }}
          >
            <span>Delete</span>
          </button>
        </div>
      )}
    </div>
  );
}

// --- panel -------------------------------------------------------------------

export function ChatPanel({
  projectId,
  projectName,
  paperId,
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
  onOpenPlan,
  onOpenSubagent,
  onOpenWorktree,
  onStartTour,
  onActiveSessionChange,
  preferredAgent,
  onPreferredAgentChange,
  children,
}: {
  projectId: string;
  projectName: string;
  /** arXiv id the project starts from — surfaces a /reproduce-paper shortcut. */
  paperId?: string | null;
  /** Back-to-projects + project name block rendered at the top of the rail. */
  railHeader?: React.ReactNode;
  /** Whether the agents rail is showing (collapsed via its own header icon). */
  railOpen: boolean;
  /** Reopen the rail (from the chat header's sidebar icon). */
  onShowRail: () => void;
  /** Settings sections replace chat; Artifacts remains a right-panel tool. */
  mainView: "chat" | SettingsTab;
  onSelectMainView: (view: "chat" | SettingsTab) => void;
  experimentsActive: boolean;
  filesActive: boolean;
  artifactsActive: boolean;
  onOpenExperiments: () => void;
  onOpenArtifacts: () => void;
  /** Open a project file in the right pane (chat tool rows are clickable).
   * `sessionId` is the chat session the click came from, so relative paths
   * can resolve against that session's worktree. */
  onOpenFile?: (path: string, sessionId?: string, line?: number, exp?: string) => void;
  /** Open a run's logs in the right pane (agent-emitted `<run>` evidence chips).
   * Run ids are globally unique, so no session context is needed. */
  onOpenRun?: (runId: string) => void;
  /** Open a plan's markdown as a right-pane tab (plan strip / plan cards). */
  onOpenPlan?: (plan: string, sessionId: string, promptId: string) => void;
  /** Open a sub-agent's transcript as a right-pane tab (spawn-row "view").
   * `sessionId` is the chat session; `spawnPartId` locates the spawn part. */
  onOpenSubagent?: (sessionId: string, spawnPartId: string) => void;
  /** Open the pinned Files home for the active session. */
  onOpenWorktree: () => void;
  /** Replay the onboarding tour (chat header help button). */
  onStartTour?: () => void;
  /** The open chat session, surfaced so the shell can scope panes to it. */
  onActiveSessionChange?: (sessionId: string | null) => void;
  /** Database-backed selection used to seed new chat sessions. */
  preferredAgent: ModelSelection | null;
  onPreferredAgentChange: (selection: ModelSelection) => Promise<void>;
  /** Middle-pane content when a settings section is active. */
  children?: React.ReactNode;
}) {
  const [sessions, setSessions] = useState<ChatSession[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [unreadSessionIds, setUnreadSessionIds] = useState<ReadonlySet<string>>(new Set());
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>("active");
  const [draft, setDraft] = useState("");
  // Pasted/dropped/uploaded attachments waiting in the composer, as data URLs.
  const [attachments, setAttachments] = useState<
    { dataUrl: string; mediaType: string; name?: string; size: number }[]
  >([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [state, dispatch] = useReducer(reducer, {
    messagesBySession: {},
    busySessions: new Set<string>(),
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
  const composerRef = useRef<HTMLTextAreaElement>(null);

  // Slash-skills: menu state is derived from the draft — open while the first
  // token is an unfinished `/command` (no whitespace yet) with matches.
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [skillIdx, setSkillIdx] = useState(0);
  const [skillMenuDismissed, setSkillMenuDismissed] = useState(false);
  // A picked skill renders as a chip on the textarea's first line
  // (Claude-desktop style); the textarea then holds only the args. send()
  // reassembles `/name args`, so the wire and transcript keep the plain-text
  // form. The chip overlays the textarea and the first line is indented past
  // it (text-indent), so long args wrap full-width beneath the chip instead
  // of being squeezed into a narrower column.
  const [pickedSkill, setPickedSkill] = useState<SkillInfo | null>(null);
  const chipRef = useRef<HTMLSpanElement>(null);
  const [chipIndent, setChipIndent] = useState(0);
  useLayoutEffect(() => {
    setChipIndent(pickedSkill && chipRef.current ? chipRef.current.offsetWidth + 8 : 0);
    syncChipScroll();
  }, [pickedSkill]);

  /** The chip belongs to the first line of *content*, so when the textarea
   * scrolls it must ride along (and clip at the wrapper) instead of sitting
   * fixed over whatever line scrolled to the top. */
  function syncChipScroll() {
    if (chipRef.current)
      chipRef.current.style.transform = `translateY(${-(composerRef.current?.scrollTop ?? 0)}px)`;
  }
  // IME guard: mid-composition text can transiently look like a full command.
  const composingRef = useRef(false);
  useEffect(() => {
    getSkills().then(setSkills).catch(() => {});
  }, []);
  const slashToken =
    !pickedSkill && draft.startsWith("/") && !/\s/.test(draft) ? draft.slice(1) : null;
  const skillMatches =
    slashToken !== null && !skillMenuDismissed
      ? skills.filter((s) => s.name.startsWith(slashToken.toLowerCase()))
      : [];
  const skillMenuOpen = skillMatches.length > 0;
  const activeSkillIdx = Math.min(skillIdx, Math.max(0, skillMatches.length - 1));
  useEffect(() => setSkillIdx(0), [slashToken]);

  function pickSkill(skill: SkillInfo) {
    setPickedSkill(skill);
    setDraft("");
    composerRef.current?.focus();
  }

  /** Backspace at the start deletes the command outright (Claude-desktop
   * behavior) — the args stay put; re-type `/` to pick another skill. */
  function removeSkillChip() {
    setPickedSkill(null);
    composerRef.current?.focus();
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
        setAttachError(`${file.name} is too large — each attachment must be under 30 MB.`);
        continue;
      }
      if (total + file.size > TOTAL_BYTES) {
        setAttachError("Attachments exceed the 40 MB total limit — remove one and try again.");
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
  const rawSelection: ModelSelection | null = openSession
    ? {
        harness: openSession.harness,
        model: sessionOverride.model ?? openSession.model,
        permissionMode: sessionOverride.permissionMode ?? openSession.permissionMode,
        reasoningLevel: sessionOverride.reasoningLevel ?? openSession.reasoningLevel,
      }
    : (selection ?? defaultSelection(harnesses));
  const activeHarness = rawSelection
    ? harnesses.find((h) => h.id === rawSelection.harness)
    : undefined;
  const opts = activeHarness?.options;
  // Reconcile the reasoning level against the *currently selected model* here
  // rather than only in the picker's `pick`. Two paths reach the composer with
  // a level nobody chose for this model: a session row stored by an older build
  // (which always wrote an explicit effort), and a stale saved preference.
  // Reconciling at the point the composer derives its state covers both, so the
  // displayed value and the value `send` transmits can never be one the model
  // rejects.
  const composerSelection: ModelSelection | null = rawSelection && {
    ...rawSelection,
    reasoningLevel: reconcileReasoning(
      activeHarness,
      rawSelection.model,
      rawSelection.reasoningLevel,
    ),
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
    setSelection(merged);
    void onPreferredAgentChange(merged).catch(() => {});
    if (openSession) setSessionOverride((cur) => ({ ...cur, ...next }));
  };
  const setPermissionMode = (id: string) => selectModel({ permissionMode: id });
  const setReasoningLevel = (id: string) => selectModel({ reasoningLevel: id });

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
    setPickedSkill(null);
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
    if (!activeId || loadedSessions.current.has(activeId)) return;
    loadedSessions.current.add(activeId);
    getChatMessages(activeId)
      .then((messages) => dispatch({ type: "seed", sessionId: activeId, messages }))
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
          .then((messages) => {
            dispatch({ type: "seed", sessionId: activeId, messages });
            if (allowRetry && msgGen.current !== gen) reseed(false);
          })
          .catch(() => {});
      };
      reseed(true);
    });
  }, [activeId, syncSessionList]);

  const messages = activeId ? (state.messagesBySession[activeId] ?? []) : [];
  const busy = activeId ? state.busySessions.has(activeId) : false;
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

  // Plan opens are stamped with the session like file opens are. Memoized
  // (along with openFileInSession and respond below) so the memoized Message
  // rows don't all re-render on every streaming tick.
  const openPlan = useMemo(
    () =>
      onOpenPlan && activeId
        ? (plan: string, promptId: string) => onOpenPlan(plan, activeId, promptId)
        : undefined,
    [onOpenPlan, activeId],
  );

  const openSubagent = useMemo(
    () =>
      onOpenSubagent && activeId
        ? (spawnPartId: string) => onOpenSubagent(activeId, spawnPartId)
        : undefined,
    [onOpenSubagent, activeId],
  );

  // File opens resolve against the active session's worktree — the agent runs
  // there, so that's where its paths point.
  const openFileInSession = useMemo(
    () =>
      onOpenFile &&
      ((path: string, line?: number, exp?: string) =>
        onOpenFile(path, activeId ?? undefined, line, exp)),
    [onOpenFile, activeId],
  );

  // Drop any unsent composer tweak when switching sessions, so it never bleeds
  // from one session's pickers onto another's.
  useEffect(() => setSessionOverride({}), [activeId]);

  // Surface the open session to the shell (Agent-scoped panes key off it).
  useEffect(() => {
    onActiveSessionChange?.(activeId);
  }, [activeId, onActiveSessionChange]);

  // Opening a session or returning from settings starts pinned at the latest messages.
  const threadMounted = mainView === "chat" && (messages.length > 0 || busy);
  useLayoutEffect(() => {
    stickToBottom.current = true;
    const el = threadRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [activeId, threadMounted]);

  // Autoscroll while pinned. Layout effect, so history seeds and streamed
  // messages land already scrolled (no flash of the top of the thread).
  useLayoutEffect(() => {
    const el = threadRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages, busy]);

  // Re-pin when the thread resizes without a message change — images loading,
  // tool rows expanding, the pane resizing.
  useEffect(() => {
    const el = threadRef.current;
    const inner = threadInnerRef.current;
    if (!el || !inner) return;
    const ro = new ResizeObserver(() => {
      if (stickToBottom.current) el.scrollTop = el.scrollHeight;
    });
    ro.observe(inner);
    ro.observe(el);
    return () => ro.disconnect();
  }, [threadMounted]);

  async function send() {
    const args = draft.trim();
    // Reassemble the picked skill chip into the plain `/name args` wire form —
    // the backend's slash expansion and the transcript both see only text.
    const text = pickedSkill ? `/${pickedSkill.name}${args ? ` ${args}` : ""}` : args;
    const pending = attachments;
    if (!text && pending.length === 0) return;
    // A pending question card owns plain typed text as a custom answer
    // (Claude-desktop behavior). This also works while the turn is HELD on
    // the card — where a new message would be rejected as busy and silently
    // dropped. A failed answer restores the draft so the text isn't lost.
    // (Auto-convert is off while a card is pending; a chip picked from the
    // menu or left over just serializes into the note text, same as typing it.)
    if (text && pendingQuestion && pending.length === 0) {
      setDraft("");
      setPickedSkill(null);
      void respond({ promptId: pendingQuestion, answers: [], note: text }).then((ok) => {
        if (!ok) setDraft((cur) => cur || text);
      });
      return;
    }
    if (busy) return;
    if (!activeHarness?.agentReady) return;
    // `composerSelection` already resolves to the open session's settings (+ any
    // unsent tweak) or, for a new session, the global preference.
    const effective = composerSelection;
    if (!effective && !activeId) return; // no harness available at all
    setDraft("");
    setPickedSkill(null);
    setAttachments([]);
    setAttachError(null);
    let sid = activeId;
    try {
      if (!sid) {
        const session = await createChatSession(projectId, effective!.harness, {
          model: effective!.model,
          permissionMode: effective!.permissionMode,
          reasoningLevel: effective!.reasoningLevel,
        });
        loadedSessions.current.add(session.id);
        setSessions((cur) => [session, ...cur]);
        setActiveId(session.id);
        sid = session.id;
      }
      dispatch({
        type: "optimisticUser",
        sessionId: sid,
        text,
        attachments: pending.map((a) => ({ url: a.dataUrl, mediaType: a.mediaType, name: a.name })),
      });
      dispatch({ type: "busy", sessionId: sid, busy: true });
      stickToBottom.current = true;
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
            permissionMode: effective.permissionMode,
            reasoningLevel: effective.reasoningLevel,
          }
        : {};
      setSessionOverride({});
      const images: ChatImageAttachment[] = pending.map((a) => ({
        mediaType: a.mediaType,
        dataBase64: a.dataUrl.slice(a.dataUrl.indexOf(",") + 1),
        name: a.name,
      }));
      await sendChatMessage(sid, text, turnOpts, images.length ? images : undefined);
    } catch (err) {
      // The message never reached a turn — put it back in the composer so a
      // retry is one keypress, whichever branch below applies.
      setDraft((cur) => cur || text);
      setAttachments((cur) => (cur.length ? cur : pending));
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
          setDraft((cur) => (cur === text ? "" : cur));
          setAttachments((cur) => (cur === pending ? [] : cur));
          return;
        }
      }
      dispatch({ type: "busy", sessionId: sid, busy: false });
      // Surface the failure instead of swallowing it: a silently dropped send
      // leaves the optimistic bubble unanswered and reads as "orx did nothing".
      // Local-only — swept by upsertMessage's LOCAL_PREFIX filter when the next
      // server user message lands (or by the reconnect reseed), and gone on
      // reload.
      dispatch({
        type: "upsertMessage",
        sessionId: sid,
        message: {
          id: `${LOCAL_PREFIX}senderr-${Date.now()}`,
          role: "assistant",
          parts: [
            {
              id: "p0",
              type: "tool",
              tool: "error",
              state: {
                status: "error",
                error: `Message not sent: ${msg}`,
              },
            },
          ],
          createdAt: Date.now(),
        },
      });
    }
  }

  function stop() {
    if (activeId) void interruptChat(activeId);
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
    const title = session.title?.trim() || "Untitled";
    if (!window.confirm(`Delete "${title}"?\n\nIts transcript is permanently removed.`)) return;
    try {
      await deleteChatSession(session.id);
    } catch (err) {
      window.alert(
        `Failed to delete "${title}": ${err instanceof Error ? err.message : String(err)}`,
      );
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
      return respondChat(sid, answer)
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
            .then((messages) => dispatch({ type: "seed", sessionId: sid, messages }))
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
    [activeId, projectId],
  );

  const visibleSessions = sessions.filter((s) => matchesFilter(sessionFilter, s.archived));
  const newTaskShortcut = /Mac|iPhone|iPad/.test(navigator.platform)
    ? "⌘ ⇧ Enter"
    : "Ctrl + Shift + Enter";
  const startNewTask = useCallback(() => {
    setSessionFilter("active");
    setActiveId(null);
    onSelectMainView("chat");
  }, [onSelectMainView]);

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
    <aside className="session-rail w-68 shrink-0 flex flex-col mt-2.5 mr-3.5 mb-2.5 ml-0 bg-background min-h-0 [&_.rail-body]:flex-1 [&_.rail-body]:min-h-0 [&_.rail-body]:overflow-y-auto [&_.rail-body]:py-1 [&_.rail-body]:px-2 floating-panel border border-border rounded-lg shadow-[0_6px_24px_color-mix(in_oklab,_var(--text)_5%,_transparent),_0_1px_4px_color-mix(in_oklab,_var(--text)_4%,_transparent)] overflow-hidden">
      {railHeader}
      {/* Workspace tools open beside chat; settings sections replace the middle pane. */}
      <nav className="rail-nav flex flex-col gap-0.5 p-2 shrink-0">
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-left [&:hover]:bg-surface [&.active]:bg-panel [&.active]:font-semibold ${experimentsActive ? "active" : ""}`}
          onClick={onOpenExperiments}
        >
          <FlaskConical size={15} />
          Experiments
        </button>
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-left [&:hover]:bg-surface [&.active]:bg-panel [&.active]:font-semibold ${filesActive ? "active" : ""}`}
          onClick={onOpenWorktree}
        >
          <FolderOpen size={15} />
          Files
        </button>
        <button
          className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-left [&:hover]:bg-surface [&.active]:bg-panel [&.active]:font-semibold ${artifactsActive ? "active" : ""}`}
          data-onboarding="nav-artifacts"
          onClick={onOpenArtifacts}
        >
          <Package size={15} />
          Artifacts
        </button>
        {SETTINGS_NAV.map((item) => (
          <button
            key={item.id}
            className={`rail-nav-item flex items-center gap-2.5 py-[7px] px-2.5 text-base text-text rounded-md text-left [&:hover]:bg-surface [&.active]:bg-panel [&.active]:font-semibold ${mainView !== "chat" && item.activeTabs.includes(mainView) ? "active" : ""}`}
            data-onboarding={item.id === "compute" ? "nav-compute" : undefined}
            onClick={() => onSelectMainView(item.id)}
          >
            {item.icon}
            {item.label}
          </button>
        ))}
      </nav>
      <div className="rail-section-head flex items-center justify-between shrink-0 pt-3.5 pr-2.5 pb-1.5 pl-4.5">
        <div className="rail-section-label p-0 text-md font-medium text-subtext">
          {SESSION_FILTERS.find((f) => f.id === sessionFilter)?.railLabel ?? "Recents"}
        </div>
        <div className="rail-section-actions flex items-center gap-0.5">
          <button
            className="rail-section-new inline-flex items-center gap-1 py-[3px] px-1.5 rounded-sm text-subtext text-xs font-medium [&:hover]:text-text [&:hover]:bg-surface tip-up [&[data-tip]::after]:top-auto [&[data-tip]::after]:bottom-[calc(100%_+_6px)]"
            data-onboarding="new-session"
            data-tip={newTaskShortcut}
            aria-keyshortcuts="Meta+Shift+Enter Control+Shift+Enter"
            onClick={startNewTask}
          >
            <Plus size={13} />
            Task
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
          <div className="rail-empty py-1.5 px-2.5 text-md text-muted">
            {sessionFilter === "archived"
              ? "No archived sessions"
              : sessions.length > 0
                ? "No active sessions"
                : "No sessions yet"}
          </div>
        )}
      </div>
    </aside>
  );

  // With the rail hidden, the header stretches to the full pane width
  // (Claude-desktop style): the reopen toggle sits in the window's top-left
  // corner with the title beside it, instead of riding the centered readable
  // column.
  const headerClass = `chat-header flex items-center gap-2 py-0 px-4 bg-background shrink-0 h-12 relative z-4 w-full max-w-readable my-0 mx-auto [&.rail-hidden]:max-w-none [&.rail-hidden]:py-0 [&.rail-hidden]:px-0.5 [&::after]:content-[''] [&::after]:absolute [&::after]:top-full [&::after]:left-0 [&::after]:right-0 [&::after]:h-6 [&::after]:bg-[linear-gradient(to_bottom,_var(--base),_transparent)] [&::after]:pointer-events-none${railOpen ? "" : " rail-hidden"}`;
  const railReopen = !railOpen && (
    <button
      className={ICON_BUTTON_CLASS_NAME}
      title="Show sidebar"
      aria-label="Show sidebar"
      onClick={onShowRail}
    >
      <PanelLeft size={15} />
    </button>
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
      {/* Header — session title on the left, right-pane view switchers on the
          right, fading into the chat below (sessions live in the rail). */}
      <div className={headerClass}>
        {railReopen}
        <div
          className={PAPER_TITLE_CLASS_NAME}
          title={activeSession ? activeSession.title?.trim() || "Untitled" : "New session"}
        >
          {activeSession ? (
            <TitleReveal
              key={activeTitleReveal ?? "static"}
              title={activeSession.title?.trim() || "Untitled"}
              animate={activeTitleReveal !== undefined}
            />
          ) : (
            "New session"
          )}
        </div>
        {onStartTour && (
          <button
            className={ICON_BUTTON_CLASS_NAME}
            data-tip="Replay tour"
            aria-label="Replay tour"
            onClick={onStartTour}
          >
            <HelpCircle size={15} />
          </button>
        )}
      </div>

      {historyLoading ? (
        <div className="chat-loading flex-1 flex items-center justify-center gap-3 text-subtext text-xl p-5 [&_.spinner]:w-5.5 [&_.spinner]:h-5.5 [&_.spinner]:border-[3px]" aria-live="polite" aria-busy="true">
          <span className={SPINNER_CLASS_NAME} />
          <span>Loading conversation…</span>
        </div>
      ) : !threadMounted ? (
        <div className="chat-empty flex-1 flex flex-col items-center justify-center @container text-text p-8 text-center [&_h2]:m-0 [&_h2]:text-5xl [&_h2]:font-medium [&_h2]:tracking-[-0.015em] [&_h2]:text-text">
          <div className="chat-empty-mark w-10.5 h-10.5 mb-5.5 [&_svg]:block [&_svg]:w-full [&_svg]:h-full">
            <BrandMark />
          </div>
          <h2>What should we research?</h2>
          <div className="chat-empty-project inline-flex items-center gap-[7px] mt-3 py-1.5 px-3 border border-border rounded-full text-subtext bg-surface text-lg font-semibold">
            <FolderOpen size={19} />
            <span>{projectName}</span>
          </div>
          <div className="chat-empty-starters grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-2.5 w-[min(100%,_620px)] mt-19 [@container((min-width:_500px))]:grid-cols-[repeat(4,_minmax(0,_1fr))] [@container((min-width:_500px))]:w-[min(100%,_720px)]">
            <button
              type="button"
              className="chat-empty-starter min-h-28 flex flex-col items-start justify-between gap-5 p-4 border border-border rounded-lg text-text bg-background shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_5%,_transparent)] text-left text-md font-medium leading-[1.35] transition-[border-color,background,translate] duration-120 ease-standard [&:hover]:border-muted [&:hover]:bg-surface [&:hover]:-translate-y-px [&.blue_svg]:text-accent-blue [&.purple_svg]:text-accent-purple [&.green_svg]:text-accent-green [&.orange_svg]:text-accent-orange blue"
              onClick={() => {
                setPickedSkill(null);
                setDraft("Explore this codebase and explain its architecture, key components, and open research questions.");
                composerRef.current?.focus();
              }}
            >
              <BookOpen size={16} />
              <span>Explore this codebase</span>
            </button>
            <button
              type="button"
              className="chat-empty-starter min-h-28 flex flex-col items-start justify-between gap-5 p-4 border border-border rounded-lg text-text bg-background shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_5%,_transparent)] text-left text-md font-medium leading-[1.35] transition-[border-color,background,translate] duration-120 ease-standard [&:hover]:border-muted [&:hover]:bg-surface [&:hover]:-translate-y-px [&.blue_svg]:text-accent-blue [&.purple_svg]:text-accent-purple [&.green_svg]:text-accent-green [&.orange_svg]:text-accent-orange purple"
              onClick={() => {
                const skill = skills.find((s) => s.name === "reproduce-paper");
                if (paperId && skill) {
                  setPickedSkill(skill);
                  setDraft(`${paperId} on `);
                } else {
                  setPickedSkill(null);
                  setDraft(
                    paperId
                      ? `/reproduce-paper ${paperId} on `
                      : "Find and summarize the research most relevant to this project.",
                  );
                }
                composerRef.current?.focus();
              }}
            >
              <GitBranch size={16} />
              <span>{paperId ? "Reproduce the linked paper" : "Review relevant literature"}</span>
            </button>
            <button
              type="button"
              className="chat-empty-starter min-h-28 flex flex-col items-start justify-between gap-5 p-4 border border-border rounded-lg text-text bg-background shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_5%,_transparent)] text-left text-md font-medium leading-[1.35] transition-[border-color,background,translate] duration-120 ease-standard [&:hover]:border-muted [&:hover]:bg-surface [&:hover]:-translate-y-px [&.blue_svg]:text-accent-blue [&.purple_svg]:text-accent-purple [&.green_svg]:text-accent-green [&.orange_svg]:text-accent-orange green"
              onClick={() => {
                setPickedSkill(null);
                setDraft("Set up and run an experiment for this project, including a baseline and meaningful variants.");
                composerRef.current?.focus();
              }}
            >
              <FlaskConical size={16} />
              <span>Run an experiment</span>
            </button>
            <button
              type="button"
              className="chat-empty-starter min-h-28 flex flex-col items-start justify-between gap-5 p-4 border border-border rounded-lg text-text bg-background shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_5%,_transparent)] text-left text-md font-medium leading-[1.35] transition-[border-color,background,translate] duration-120 ease-standard [&:hover]:border-muted [&:hover]:bg-surface [&:hover]:-translate-y-px [&.blue_svg]:text-accent-blue [&.purple_svg]:text-accent-purple [&.green_svg]:text-accent-green [&.orange_svg]:text-accent-orange orange"
              onClick={() => {
                setPickedSkill(null);
                setDraft("Analyze the latest experiment results and recommend the most useful next iteration.");
                composerRef.current?.focus();
              }}
            >
              <ChartSpline size={16} />
              <span>Analyze results</span>
            </button>
          </div>
        </div>
      ) : (
        <div
          className="chat-thread flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges]"
          ref={threadRef}
          onScroll={(e) => {
            const el = e.currentTarget;
            stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
          }}
        >
          <div className="chat-thread-inner max-w-readable my-0 mx-auto pt-4 px-4 pb-8 flex flex-col gap-4" ref={threadInnerRef}>
            <Transcript
              messages={messages}
              onOpenFile={openFileInSession}
              onOpenRun={onOpenRun}
              onRespond={respond}
              onOpenPlan={openPlan}
              onOpenSubagent={openSubagent}
              skills={skills}
            />
            {busy &&
              (awaitingInput ? (
                <div className="working flex items-center gap-2 text-subtext text-md pt-0.5 px-0 pb-2 [&.awaiting]:italic awaiting">Waiting for your input…</div>
              ) : (
                <div className="working flex items-center gap-2 text-subtext text-md pt-0.5 px-0 pb-2 [&.awaiting]:italic">
                  <span className={SPINNER_CLASS_NAME} /> Working…
                </div>
              ))}
          </div>
        </div>
      )}

      {/* Docked while a plan awaits a decision, so the approval controls never
          scroll away. Actions mirror the (now compact) inline card's wire. */}
      <div className="composer pt-0 px-3 pb-3 shrink-0 relative z-4 bg-background w-full max-w-readable my-0 mx-auto [&::before]:content-[''] [&::before]:absolute [&::before]:bottom-full [&::before]:left-0 [&::before]:right-0 [&::before]:h-6 [&::before]:bg-[linear-gradient(to_top,_var(--base),_transparent)] [&::before]:pointer-events-none [&_textarea]:border-0 [&_textarea]:bg-none [&_textarea]:bg-transparent [&_textarea]:resize-none [&_textarea]:pt-2.5 [&_textarea]:px-3 [&_textarea]:pb-1 [&_textarea]:text-base [&_textarea]:field-sizing-content [&_textarea]:min-h-18 [&_textarea]:max-h-45">
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
              activeSession ? HARNESS_LABELS[activeSession.harness] : "The agent"
            }
            onView={() => openPlan?.(pendingPlan.plan, pendingPlan.promptId)}
            onApprove={(resumeMode) =>
              respond({ promptId: pendingPlan.promptId, approve: true, resumeMode })
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
        <div className="composer-box relative flex flex-col border border-border rounded-md bg-background" data-onboarding="composer">
          {activeHarness && !activeHarness.agentReady && (
            <div className="composer-harness-warning py-2 px-3 text-subtext text-xs leading-normal border-b border-b-border-variant [&_strong]:text-accent-amber [&_strong]:font-medium [&_code]:font-mono [&_code]:text-text">
              <strong>{activeHarness.name} is unavailable.</strong>{" "}
              {activeHarness.agentNote ? renderNote(activeHarness.agentNote) : "Re-check its setup."}
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
          {attachments.length > 0 && (
            <div className="composer-attachments flex flex-wrap gap-1.5 pt-2 px-3 pb-0">
              {attachments.map((a, i) => {
                const remove = () =>
                  setAttachments((cur) => cur.filter((_, j) => j !== i));
                return a.mediaType === "application/pdf" ? (
                  <div key={i} className="attachment-file [&_button]:absolute [&_button]:-top-[5px] [&_button]:-right-[5px] [&_button]:inline-flex [&_button]:items-center [&_button]:justify-center [&_button]:w-4 [&_button]:h-4 [&_button]:p-0 [&_button]:border [&_button]:border-border [&_button]:rounded-full [&_button]:bg-surface [&_button]:text-text [&_button]:cursor-pointer [&_button:hover]:bg-text [&_button:hover]:text-background relative inline-flex items-center gap-2 max-w-55 py-2 px-2.5 border border-border rounded-sm text-text bg-surface [&_svg]:shrink-0 [&_svg]:text-muted" title={a.name}>
                    <FileText size={22} />
                    <span className="attachment-file-name overflow-hidden text-ellipsis whitespace-nowrap text-sm">{a.name ?? "document.pdf"}</span>
                    <button title="Remove file" aria-label="Remove file" onClick={remove}>
                      <X size={11} />
                    </button>
                  </div>
                ) : (
                  <div key={i} className="attachment-thumb relative [&_img]:w-13 [&_img]:h-13 [&_img]:object-cover [&_img]:border [&_img]:border-border [&_img]:rounded-sm [&_img]:block [&_button]:absolute [&_button]:-top-[5px] [&_button]:-right-[5px] [&_button]:inline-flex [&_button]:items-center [&_button]:justify-center [&_button]:w-4 [&_button]:h-4 [&_button]:p-0 [&_button]:border [&_button]:border-border [&_button]:rounded-full [&_button]:bg-surface [&_button]:text-text [&_button]:cursor-pointer [&_button:hover]:bg-text [&_button:hover]:text-background">
                    <img src={a.dataUrl} alt="pasted" />
                    <button title="Remove image" aria-label="Remove image" onClick={remove}>
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
          <div className="composer-input relative flex overflow-hidden [&_textarea]:flex-1">
            {pickedSkill && (
              // Inert like inline text: clicks fall through to the textarea
              // (pointer-events: none); Backspace at the start removes it.
              <span ref={chipRef} className="skill-chip inline-flex items-center py-px px-[7px] font-mono text-md font-medium text-primary bg-primary-subtle border border-border-variant rounded-sm composer-chip absolute top-[9px] left-3 z-1 pointer-events-none">
                /{pickedSkill.name}
              </span>
            )}
            <textarea
              ref={composerRef}
              value={draft}
              style={pickedSkill ? { textIndent: chipIndent } : undefined}
              onScroll={syncChipScroll}
              placeholder={
                // A pending question card owns typed text (see send()); say so.
                // With a chip active, the skill's arg hint says what to type —
                // and when the project already has a paper attached, the paper
                // part of the paper-reproduction skills defaults to it, so mark
                // just that part optional (compute is still expected).
                // Otherwise follow `composerSelection` so the name tracks the
                // picker for a new session and the open session once one exists.
                pendingQuestion
                  ? "Type a custom answer…"
                  : pickedSkill
                    ? ["reproduce-paper", "paper-to-marimo"].includes(pickedSkill.name) &&
                      paperId
                      ? `[paper — optional, defaults to ${paperId}] on [compute]`
                      : pickedSkill.argHint
                    : composerSelection
                      ? activeHarness?.agentReady
                        ? `Message ${HARNESS_LABELS[composerSelection.harness]}… ( / for skills)`
                        : `${HARNESS_LABELS[composerSelection.harness]} is unavailable — open the model picker`
                      : "Ask the research agent… ( / for skills)"
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
                // Auto-convert a typed/pasted full `/name ` into the chip the
                // moment the space lands. Known names only — unknown `/foo`
                // stays plain text (server-side pass-through contract). Not
                // while a question card is pending (its answer is a note, never
                // skill-expanded) and not mid-IME-composition.
                if (!pickedSkill && !pendingQuestion && !composingRef.current) {
                  const m = v.match(/^\/(\S+)\s([\s\S]*)$/);
                  const hit = m && skills.find((s) => s.name === m[1].toLowerCase());
                  if (hit) {
                    setPickedSkill(hit);
                    setDraft(m[2]);
                    setSkillMenuDismissed(false);
                    return;
                  }
                }
                setDraft(v);
                setSkillMenuDismissed(false);
              }}
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
                  if (e.key === "Enter" || e.key === "Tab") {
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
                // Backspace at the very start deletes the command chip.
                // (Escape deliberately doesn't touch the chip — it's the
                // stop-the-turn gesture, see the document listener above.)
                if (
                  pickedSkill &&
                  e.key === "Backspace" &&
                  e.currentTarget.selectionStart === 0 &&
                  e.currentTarget.selectionEnd === 0
                ) {
                  e.preventDefault();
                  removeSkillChip();
                  return;
                }
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void send();
                }
              }}
            />
          </div>
          <div className="composer-actions flex justify-end items-center gap-2 pt-1.5 px-2 pb-2">
            {/* Bottom-left: permission mode + literature sources. */}
            <OptionPicker
              choices={activeHarness?.agentReady ? (opts?.permissionModes ?? []) : []}
              value={composerSelection?.permissionMode ?? null}
              defaultId={opts?.defaultPermissionMode ?? null}
              header="Mode"
              align="left"
              variant="pill"
              numbered
              title="Permission mode for this chat"
              onSelect={setPermissionMode}
            />
            <LitSourcesPicker />
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
            <button
              type="button"
              className="composer-attach inline-flex items-center justify-center w-7.5 h-7.5 rounded-md text-muted cursor-pointer transition-[background,color] duration-100 ease-standard [&:hover]:bg-surface [&:hover]:text-text"
              title="Attach a PDF or image"
              aria-label="Attach a PDF or image"
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip size={16} />
            </button>
            <div style={{ flex: 1 }} />
            {/* Bottom-right: model, reasoning level, then context meter. The
                picker reflects the open session (harness locked once it exists);
                the global default only applies before the first message. */}
            <ModelPicker
              value={composerSelection}
              onSelect={selectModel}
              onHarnesses={setHarnesses}
              lockHarness={!!openSession}
            />
            <OptionPicker
              choices={activeHarness?.agentReady ? reasoning.choices : []}
              value={composerSelection?.reasoningLevel ?? null}
              defaultId={reasoning.defaultId}
              header="Reasoning"
              align="right"
              variant="bare"
              title="Reasoning level for this chat — Default sends no override, so the harness CLI's own configured effort applies"
              onSelect={setReasoningLevel}
            />
            <ContextMeter usage={openSession?.contextUsage} />
            {busy && !pendingQuestion ? (
              // Stop whenever the turn is busy and typed text has nowhere to
              // go — actively streaming, or held on a plan/permission card
              // (their cards are the affordance; send() can't service them).
              // Send stays only when it actually works: idle, or a held
              // QUESTION card that owns typed text.
              <button className="send-btn inline-flex items-center justify-center w-8 h-8 rounded-md bg-primary text-background transition-[background,opacity] duration-100 ease-standard [&:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--primary)_88%,_var(--text))] [&:disabled]:opacity-40 [&:disabled]:cursor-default [&.stop]:bg-surface [&.stop]:text-text [&.stop:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--surface)_88%,_var(--text))] stop" title="Stop" aria-label="Stop" onClick={stop}>
                <X size={16} />
              </button>
            ) : (
              <button
                className="send-btn inline-flex items-center justify-center w-8 h-8 rounded-md bg-primary text-background transition-[background,opacity] duration-100 ease-standard [&:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--primary)_88%,_var(--text))] [&:disabled]:opacity-40 [&:disabled]:cursor-default [&.stop]:bg-surface [&.stop]:text-text [&.stop:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--surface)_88%,_var(--text))]"
                title="Send"
                aria-label="Send"
                onClick={() => void send()}
                disabled={
                  !activeHarness?.agentReady ||
                  (!pickedSkill && !draft.trim() && attachments.length === 0)
                }
              >
                <CornerDownLeft size={16} />
              </button>
            )}
          </div>
        </div>
      </div>
      </section>
    </>
  );
}
