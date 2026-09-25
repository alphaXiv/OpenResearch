import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { MessageSquareQuote, Send, Square } from "lucide-react";
import { interruptChat, respondChat, sendChatMessage, type ChatPart, type ChatSession, type PromptAnswer } from "../api";
import { getChatMessagesQuery } from "../queries/chat";
import { activePath } from "../transcriptTree";
import { m } from "../paraglide/messages.js";
import { Md } from "./Md";
import { PromptCard } from "./ChatPanel";
import { Button, IconButton, Spinner } from "./ui";

function visibleParts(parts: ChatPart[]) {
  return parts.filter((part) =>
    (part.type === "text" && part.text?.trim()) ||
    (part.type === "prompt" && !(part.prompt?.kind === "permission" && part.prompt.resolved)) ||
    part.type === "tool",
  );
}

/** A disposable conversation alongside its parent, not a second main-chat view. */
export function SideChatPane({ session, parentTitle, promoting, onPromote }: {
  session: ChatSession;
  parentTitle: string;
  promoting: boolean;
  onPromote: () => void;
}) {
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [responding, setResponding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const threadEnd = useRef<HTMLDivElement>(null);
  const history = useQuery(getChatMessagesQuery(session.id));
  const messages = useMemo(
    () => activePath(history.data?.messages ?? [], history.data?.activeLeafId ?? null),
    [history.data],
  );

  useEffect(() => {
    setDraft("");
    setError(null);
  }, [session.id]);
  useEffect(() => {
    threadEnd.current?.scrollIntoView({ block: "end" });
  }, [messages]);

  async function send() {
    const text = draft.trim();
    if (!text || sending || session.busy) return;
    setDraft("");
    setError(null);
    setSending(true);
    try {
      await sendChatMessage(session.id, text);
      await history.refetch();
    } catch (cause) {
      setDraft(text);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSending(false);
    }
  }

  async function stop() {
    try {
      await interruptChat(session.id);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function respond(answer: PromptAnswer) {
    if (responding) return;
    setResponding(true);
    setError(null);
    try {
      await respondChat(session.id, answer);
      await history.refetch();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setResponding(false);
    }
  }

  function onComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div className="shrink-0 border-b border-border px-4 py-3">
        <div className="flex items-center gap-2 text-sm font-medium text-text">
          <MessageSquareQuote size={16} />
          <span className="min-w-0 truncate">{session.title?.trim() || m.chat_side_new()}</span>
          <span className="flex-1" />
          <Button size="small" onClick={onPromote} disabled={promoting}>{m.chat_side_keep()}</Button>
        </div>
        <p className="mt-1 mb-0 truncate text-xs text-subtext" title={parentTitle}>{parentTitle}</p>
        <p className="mt-1 mb-0 text-xs text-subtext">{m.chat_side_expires()}</p>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-5" role="log" aria-label={m.chat_side_new()}>
        {history.isPending ? (
          <div className="flex justify-center py-10"><Spinner /></div>
        ) : history.isError ? (
          <div role="alert" className="text-sm text-accent-red">{history.error.message}</div>
        ) : messages.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-subtext">
            <MessageSquareQuote size={28} />
            <p className="m-0 max-w-64 text-sm">{m.chat_side_ask()}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-5">
            {messages.map((message) => {
              const parts = visibleParts(message.parts);
              if (!parts.length) return null;
              return (
                <div key={message.id} className={`flex flex-col gap-2 ${message.role === "user" ? "items-end" : "items-start"}`}>
                  <span className="text-xs text-subtext">{message.role === "user" ? m.chat_export_you() : m.chat_the_agent()}</span>
                  <div className={`max-w-full text-sm ${message.role === "user" ? "rounded-xl bg-surface px-3 py-2.5" : "w-full"}`}>
                    {parts.map((part) => part.type === "prompt"
                      ? <PromptCard key={part.id} part={part} onRespond={(answer) => void respond(answer)} />
                      : part.type === "tool"
                        ? <div key={part.id} className="my-1 text-xs text-subtext">{part.state?.title || part.tool || m.chat_panel_used_tools()}</div>
                        : <Md key={part.id} text={part.text!} />)}
                  </div>
                </div>
              );
            })}
            {session.busy && <div className="text-sm text-subtext">{m.chat_thinking()}…</div>}
            <div ref={threadEnd} />
          </div>
        )}
      </div>
      <div className="shrink-0 border-t border-border bg-background p-3">
        {error && <p role="alert" className="mt-0 mb-2 text-xs text-accent-red">{error}</p>}
        <div className="flex items-end gap-2 rounded-xl border border-border bg-surface px-2 py-1.5">
          <textarea
            className="max-h-40 min-h-12 min-w-0 flex-1 resize-none border-0 bg-transparent px-1 py-1 text-sm text-text outline-none placeholder:text-muted"
            aria-label={m.chat_side_ask()}
            placeholder={m.chat_side_ask()}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onComposerKeyDown}
            rows={2}
          />
          {session.busy ? (
            <IconButton aria-label={m.chat_panel_stop()} title={m.chat_panel_stop()} onClick={() => void stop()}><Square size={16} /></IconButton>
          ) : (
            <IconButton aria-label={m.chat_panel_send()} title={m.chat_panel_send()} onClick={() => void send()} disabled={!draft.trim() || sending}><Send size={16} /></IconButton>
          )}
        </div>
      </div>
    </div>
  );
}
