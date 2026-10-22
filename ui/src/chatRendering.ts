import type { ChatMessage, ChatPart, ChatSession } from "./api";

export function isParentUpdate(part: ChatPart): boolean {
  return part.tool?.toLowerCase() === "subagenthandback" || part.tool === "subagent" && part.state?.input?.kind === "interacted"
    && part.id.includes(":") && !part.children?.length;
}

export function parentUpdateText(part: ChatPart): string | null {
  const message = part.state?.input?.message;
  return isParentUpdate(part) && typeof message === "string" && message.trim() ? message : null;
}

export function agentTranscript(messages: ChatMessage[], eventId: string): ChatPart | null {
  const events: ChatPart[] = [];
  function collect(parts: ChatPart[]) {
    for (const part of parts) {
      events.push(part);
      if (part.children) collect(part.children);
    }
  }
  for (const message of messages) collect(message.parts);
  const selected = events.find((part) => part.id === eventId);
  if (!selected) return null;
  if (isParentUpdate(selected)) return selected;
  const threadIds = (part: ChatPart): string[] => {
    const ids = part.state?.input?.agentThreadIds;
    return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : [];
  };
  const targets = threadIds(selected);
  if (targets.length === 0) return selected;
  const belongsToAgent = (part: ChatPart) => targets.some((id) => part.id.startsWith(`${id}:`));
  const related = events.filter((part) => part === selected
    || threadIds(part).some((id) => targets.includes(id))
    || part.children?.some(belongsToAgent));
  const children = new Map<string, ChatPart>();
  for (const event of related) {
    for (const child of event.children ?? []) {
      if (belongsToAgent(child)) children.set(child.id, child);
    }
  }
  const latest = related.at(-1) ?? selected;
  return {
    ...selected,
    state: latest.state && {
      ...latest.state,
      status: related.some((event) => event.state?.status === "running") ? "running" : latest.state.status,
    },
    children: [...children.values()],
  };
}

export function hasPendingInput(messages: ChatMessage[], busy: boolean): boolean {
  return messages.some((message) => message.parts.some((part) => {
    const prompt = part.prompt;
    return part.type === "prompt" && prompt && !prompt.resolved
      && (prompt.nativeId ? busy : prompt.kind === "question");
  }));
}

export interface QuestionReplies {
  textById: ReadonlyMap<string, string>;
  questionById: ReadonlyMap<string, string>;
  prompts: ReadonlySet<string>;
}

// Codex stores async replies with a question preamble intended for the agent.
export function questionReplies(messages: ChatMessage[]): QuestionReplies {
  const textById = new Map<string, string>();
  const questionById = new Map<string, string>();
  const prompts = new Set<string>();
  const pending = new Map<string, { id: string; answer: string; question: string }>();
  const originalQuestions = new Map<string, string>();
  for (const part of messages.flatMap((message) => message.parts)) {
    const questions = part.type === "tool" && part.state?.input?.questions;
    if (!Array.isArray(questions)) continue;
    for (const question of questions) {
      if (question && typeof question === "object" && "header" in question && typeof question.header === "string"
        && "question" in question && typeof question.question === "string") {
        originalQuestions.set(`${question.header}\n\n${question.question}`, question.question);
      }
    }
  }
  const matchReply = (id: string, text: string) => {
    const reply = pending.get(text);
    if (!reply) return;
    textById.set(id, reply.answer);
    questionById.set(id, reply.question);
    prompts.add(reply.id);
    pending.delete(text);
  };
  for (const message of messages) {
    if (message.role === "user") {
      matchReply(message.id, message.parts.filter((part) => part.type === "text").map((part) => part.text ?? "").join("\n"));
      continue;
    }
    for (const part of message.parts) {
      const prompt = part.prompt;
      if (prompt?.kind === "question" && prompt.question) {
        questionById.set(part.id, originalQuestions.get(prompt.question) ?? prompt.question);
      }
      if (part.type === "prompt" && prompt?.kind === "question" && prompt.resolved && !prompt.nativeId && prompt.question) {
        const answer = [(prompt.answers ?? []).join(", "), prompt.note].filter(Boolean).join("\n\n");
        if (answer) pending.set(`In response to: ${prompt.question}\n\n${answer}`, { id: part.id, answer, question: prompt.question });
      } else if (part.type === "steer" && part.text) {
        matchReply(part.id, part.text);
      }
    }
  }
  return { textById, questionById, prompts };
}

/** Whether a part paints anything in the transcript. */
export function partIsVisible(part: ChatPart, activePermissionId?: string | null): boolean {
  // The persisted marker still delimits stopped turns for history consumers.
  if (part.type === "tool" && part.tool?.toLowerCase() === "interrupted") return false;
  if (part.type === "prompt") {
    if (!part.prompt) return false;
    if (part.prompt.kind === "permission") {
      if (part.prompt.resolved) return false;
      // Without a selected prompt, keep unresolved permissions visible as tail boundaries.
      if (activePermissionId !== undefined) return part.id === activePermissionId;
    }
    return true;
  }
  // Hidden reasoning must not displace a visible tool tail during brief thinking bursts.
  if (part.type === "reasoning") return false;
  if (part.type === "text") return Boolean(part.text);
  return true;
}

export function isTurnStatusPart(part: ChatPart): boolean {
  return part.id === "turn-retry" || part.id === "turn-recovery";
}

export function withoutDuplicateTurnError(parts: ChatPart[], turnStatus: ChatPart | undefined): ChatPart[] {
  const last = parts.at(-1);
  return turnStatus?.id === "turn-recovery" && last?.type === "tool" && (last.tool === "error" || last.tool === "api_error")
    && last.state?.error && last.state.error === turnStatus.state?.error ? parts.slice(0, -1) : parts;
}

export function splitResponseTurn(message: ChatMessage) {
  const usageLimit = message.parts.find((part) => part.type === "tool" && isUsageLimitPart(part));
  const turnStatus = message.parts.find(isTurnStatusPart) ?? usageLimit;
  const turnParts = message.parts.filter((part) => part !== turnStatus && !(usageLimit && isUsageLimitPart(part)));
  const regularParts = withoutDuplicateTurnError(turnParts, turnStatus);
  return {
    message: { ...message, parts: regularParts },
    turnStatus,
    usageLimit,
    temporaryApiError: regularParts.length < turnParts.length && turnParts.at(-1)?.tool === "api_error",
  };
}

/** The last visible part, when it is a non-errored tool. */
export function partsTailToolId(parts: ChatPart[]): string | null {
  for (let index = parts.length - 1; index >= 0; index--) {
    const part = parts[index];
    if (part.type === "steer" || isTurnStatusPart(part) || !partIsVisible(part)) continue;
    if (part.type !== "tool" || part.state?.status === "error") return null;
    return part.id;
  }
  return null;
}

export function streamTailTool(messages: ChatMessage[]): { messageId: string; toolId: string } | null {
  const message = messages.at(-1);
  if (message?.role !== "assistant") return null;
  const toolId = partsTailToolId(message.parts);
  return toolId ? { messageId: message.id, toolId } : null;
}

export function streamTailIsText(messages: ChatMessage[]): boolean {
  const message = messages.at(-1);
  if (message?.role !== "assistant") return false;
  for (let index = message.parts.length - 1; index >= 0; index--) {
    const part = message.parts[index];
    if (part.type === "steer" || isTurnStatusPart(part)) continue;
    // Hidden reasoning ends a text tail so Thinking can show while generation pauses.
    return part.type === "text" && Boolean(part.text);
  }
  return false;
}

export function unreadAfterBusyChange(
  current: ReadonlySet<string>, previousBusy: ReadonlySet<string>, busy: ReadonlySet<string>,
  sessions: ChatSession[], visibleSessionId: string | null,
): ReadonlySet<string> {
  const finished = [...previousBusy].filter((id) => !busy.has(id)
    && sessions.some((session) => session.id === id) && id !== visibleSessionId);
  if (finished.length === 0 && !(visibleSessionId && current.has(visibleSessionId))) return current;
  const next = new Set(current);
  for (const id of finished) next.add(id);
  if (visibleSessionId) next.delete(visibleSessionId);
  return next;
}

export function splitTurnParts(parts: ChatPart[], streaming: boolean): { work: ChatPart[]; answer: ChatPart[] } {
  // Keep unanswered prompts in the visible conversation.
  if (parts.some((part) => part.type === "prompt" && !part.prompt?.resolved)) {
    return { work: [], answer: parts };
  }
  let finalIndex = parts.findIndex((part) => part.type === "text" && part.phase === "final_answer");
  if (finalIndex < 0 && !streaming && !parts.some((part) => part.phase)) {
    // Older transcripts have no phase: only the trailing text can be the answer.
    for (let index = parts.length - 1; index >= 0; index--) {
      const part = parts[index];
      if (part.type === "reasoning") continue;
      if (part.type !== "text") break;
      if (part.text) finalIndex = index;
    }
  }
  return finalIndex < 0 || (!streaming && !parts.slice(finalIndex).some((part) => partIsVisible(part))) || !parts.slice(0, finalIndex).some((part) => partIsVisible(part))
    ? { work: [], answer: parts }
    : { work: parts.slice(0, finalIndex), answer: parts.slice(finalIndex) };
}

export function groupResponseMessages(messages: ChatMessage[], previousGroups: ChatMessage[][] = []): ChatMessage[][] {
  const groups: ChatMessage[][] = [];
  for (const message of messages) {
    const previous = groups.at(-1);
    if (message.role === "assistant" && previous?.[0].role === "assistant") {
      previous.push(message);
    } else {
      groups.push([message]);
    }
  }
  return groups.map((group, index) => {
    const previous = previousGroups[index];
    return previous?.length === group.length && group.every((message, i) => message === previous[i])
      ? previous : group;
  });
}

export function splitResponseParts(messages: ChatMessage[], streaming: boolean, replies?: QuestionReplies) {
  const questions = messages.flatMap((message) => message.parts.filter((part) => part.prompt?.kind === "question"));
  const turns = messages.map((original, index) => {
    const parts = original.parts.filter((part) => {
      if (part.type !== "tool" || !["question", "askuserquestion", "request_user_input", "request_user_input_async"].includes(part.tool?.toLowerCase() ?? "")) return true;
      const asked = part.state?.input?.questions;
      if (!Array.isArray(asked) || asked.length === 0) return true;
      const represented = asked.every((item) => {
        if (!item || typeof item !== "object") return false;
        const text = "question" in item ? item.question : "title" in item ? item.title : undefined;
        return typeof text === "string" && questions.some((question) => question.prompt?.question?.split(/\n\s*\n/).includes(text));
      });
      return !represented || (part.state?.status === "error" && part.state.error !== "The user dismissed this question");
    });
    const message = { ...original, parts };
    const active = streaming && index === messages.length - 1;
    const split = splitTurnParts(message.parts, active);
    // A continuation's commentary belongs to activity until it has an answer.
    if (messages.length > 1 && active && !message.parts.some((part) =>
      (part.type === "text" && part.phase === "final_answer" && Boolean(part.text))
      || (part.type === "prompt" && !part.prompt?.resolved))) {
      return { message, work: message.parts, answer: [] };
    }
    return { message, ...split };
  });
  const current = [...turns].reverse().find((turn) => turn.answer.some((part) => partIsVisible(part)));
  const work = turns.map((turn) => ({
    id: turn.message.id,
    parts: turn === current ? turn.work : [...turn.work, ...turn.answer],
  }));
  const answeredQuestion = (part: ChatPart) => (part.type === "steer" && replies?.textById.has(part.id)) || (part.type === "prompt"
    && part.prompt?.kind === "question" && part.prompt.resolved
    && ((part.prompt.answers?.length ?? 0) > 0 || Boolean(part.prompt.note) || part.prompt.approved === false));
  return {
    answer: [...work.flatMap((turn) => turn.parts.filter(answeredQuestion)), ...(current?.answer ?? [])],
    answerMessageId: current?.message.id,
    work: work.map((turn) => ({
      ...turn,
      parts: turn.parts.filter((part) => !answeredQuestion(part)),
    })).filter((turn) => turn.parts.some((part) => partIsVisible(part))),
  };
}

function textPartsText(parts: ChatPart[]): string {
  return parts
    .flatMap((part) => {
      const text = part.type === "text" ? part.text?.trim() : undefined;
      return text ? [text] : [];
    })
    .join("\n\n");
}

/** The answer an assistant turn gave, without its commentary or tool work. */
export function responseText(message: ChatMessage): string {
  return textPartsText(splitTurnParts(message.parts, false).answer);
}

export function lastResponseText(messages: ChatMessage[]): string {
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index].role !== "assistant") continue;
    const text = responseText(messages[index]);
    if (text) return text;
  }
  return "";
}

/** `null` when the chat has no text to export yet. */
export function transcriptMarkdown(
  title: string,
  messages: ChatMessage[],
  labels: { user: string; assistant: string },
): string | null {
  const sections = messages.flatMap((message) => {
    const text = textPartsText(message.parts);
    if (!text) return [];
    return [`## ${message.role === "user" ? labels.user : labels.assistant}\n\n${text}`];
  });
  if (sections.length === 0) return null;
  return [`# ${title}`, ...sections].join("\n\n") + "\n";
}

export function transcriptFileName(title: string): string {
  const slug = title
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .slice(0, 80)
    .replace(/^-+|-+$/g, "");
  return `${slug || "chat"}.md`;
}

export function isModelAccessLimitPart(part: ChatPart): boolean {
  return part.type === "tool" && part.tool === "error"
    && /^(?:(?:ActionRequiredError:\s*)?Named models unavailable\b|ZCode turn failed:\s*1113\b)/i.test(part.state?.error?.trim() ?? "");
}

export function isUsageLimitPart(part: ChatPart): boolean {
  if (isModelAccessLimitPart(part)) return true;
  if (part.state?.input?.errorKind === "claude_usage_limit") return true;
  const text = part.type === "text" ? part.text : part.tool === "error" ? part.state?.error : null;
  if (part.type === "tool" && part.tool === "error" && text
    && /usageLimitExceeded|rateLimitExceeded|insufficient_quota|(?:usage|rate|session) limit|(?:exceeded|exhausted) (?:your |the |current )*quota|insufficient (?:credits|balance)|(?:credit|quota)[ _-](?:exhausted|exceeded)/i.test(text)) return true;
  // Older Claude transcripts stored the synthetic quota notice as text and errors.
  return Boolean(text && ((/^(?:claude: )?you(?:'ve| have) reached your .+ limit\./i.test(text.trim())
    && text.includes("claude.ai/settings/usage"))
    || /^(?:claude: )?you(?:'ve| have) hit your session limit · resets /i.test(text.trim())));
}
