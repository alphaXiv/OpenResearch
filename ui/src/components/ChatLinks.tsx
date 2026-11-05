import { createContext, useContext, type ReactNode } from "react";
import { MessageSquare } from "lucide-react";
import { useQuery } from "@tanstack/react-query";

import type { HarnessId } from "../api";
import { useWorkspace } from "../workspace";
import { refLabel } from "../agentMessages";
import { m } from "../paraglide/messages.js";
import { HarnessLogo } from "./HarnessLogo";
import { LitSourceLogo } from "./LitSourceLogo";

/** What a transcript links to when an agent names a message or chat by id. */
export type ChatLinks = {
  /** A chat in this project, or undefined for one elsewhere. */
  chat: (sessionId: string) => { title: string; harness: HarnessId } | undefined;
  onOpenChat: (sessionId: string) => void;
  onOpenMessage: (cited: string) => void;
};
export const ChatLinksContext = createContext<ChatLinks | null>(null);

function useCitedMessage(cited: string) {
  const { citedMessageQuery } = useWorkspace().queries;
  return useQuery(citedMessageQuery(cited)).data;
}

/** A cited message's first words, for a one-line preview. */
export function CitedWords({ cited }: { cited: string }) {
  return <span className="text-subtext">{useCitedMessage(cited)?.preview || refLabel(cited)}</span>;
}

const PILL_CLASS = "context-pill inline-flex max-w-72 items-center gap-1 rounded-md border border-border-variant bg-panel px-1.5 py-px align-middle text-sm leading-[1.3] text-text [&_svg]:text-subtext";

/** A message ref or chat id as a link to it, or `fallback` when nothing here can open it. */
export function CitedRef({ cited, fallback }: { cited: string; fallback: ReactNode }) {
  const links = useContext(ChatLinksContext);
  if (!links) return fallback;
  if (cited.startsWith("msg_")) return <MessageRefPill cited={cited} />;
  const chat = links.chat(cited);
  if (!chat) return fallback;
  return (
    <button type="button" className={`${PILL_CLASS} cursor-pointer hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus`} onClick={() => links.onOpenChat(cited)}>
      <HarnessLogo harness={chat.harness} size={12} />
      <span className="min-w-0 truncate">{chat.title}</span>
    </button>
  );
}

/** A paper a message cites, as a pill linking to its alphaXiv page. */
export function PaperPill({ title, url }: { title: string; url: string }) {
  return (
    <a href={url} target="_blank" rel="noreferrer" title={url} className={`${PILL_CLASS} no-underline hover:bg-surface`}>
      <LitSourceLogo source="alphaxiv" size={12} decorative />
      <span className="min-w-0 truncate">{title}</span>
    </a>
  );
}

/** A cited message as an inline pill: who wrote it and its first words. */
export function MessageRefPill({ cited }: { cited: string }) {
  const links = useContext(ChatLinksContext);
  const data = useCitedMessage(cited);
  const words = data?.preview || refLabel(cited);
  const content = (
    <>
      {data?.harness
        ? <HarnessLogo harness={data.harness} size={12} />
        : <MessageSquare size={12} aria-hidden="true" className="shrink-0" />}
      <span className="min-w-0 truncate">{words}</span>
    </>
  );
  const className = `message-ref ${PILL_CLASS}`;
  if (!links) return <span className={className}>{content}</span>;
  return (
    <button
      type="button"
      className={`${className} cursor-pointer hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus`}
      aria-label={m.chat_cited_message({ ref: words })}
      onClick={() => links.onOpenMessage(cited)}
    >
      {content}
    </button>
  );
}
