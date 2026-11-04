import { createContext, useContext } from "react";
import { MessageSquare } from "lucide-react";
import { useQuery } from "@tanstack/react-query";

import { citedMessage, type HarnessId } from "../api";
import { refLabel } from "../agentMessages";
import { m } from "../paraglide/messages.js";
import { HarnessLogo } from "./HarnessLogo";

/** What a transcript links to when an agent names a message or chat by id. */
export type ChatLinks = {
  /** A chat in this project, or undefined for one elsewhere. */
  chat: (sessionId: string) => { title: string; harness: HarnessId } | undefined;
  onOpenChat: (sessionId: string) => void;
  onOpenMessage: (cited: string) => void;
};
export const ChatLinksContext = createContext<ChatLinks | null>(null);

function useCitedMessage(cited: string) {
  return useQuery({
    queryKey: ["cited-message", cited],
    queryFn: () => citedMessage(cited),
    staleTime: Infinity,
    retry: false,
  }).data;
}

/** A cited message's first words, for a one-line preview. */
export function CitedWords({ cited }: { cited: string }) {
  return <span className="text-subtext">{useCitedMessage(cited)?.preview || refLabel(cited)}</span>;
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
  const className = "message-ref inline-flex max-w-72 items-center gap-1 rounded-md border border-border-variant bg-panel px-1.5 py-px align-middle text-sm leading-[1.3] text-text [&_svg]:text-subtext";
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
