import { useContext, useState, type ComponentProps } from "react";
import { ChevronDown, CircleAlert, CircleCheck, CircleX, MessagesSquare, TriangleAlert } from "lucide-react";
import { useQuery } from "@tanstack/react-query";

import { fmtDuration, type ChatPart, type OrxSource } from "../api";
import { useWorkspace } from "../workspace";
import { CITED_ID, firstLine } from "../agentMessages";
import { m } from "../paraglide/messages.js";
import { tabOpenGestureHandlers } from "../tabPreview";
import { ChatLinksContext, CitedWords } from "./ChatLinks";
import { FileScopeContext } from "./FileReference";
import { HarnessLogo } from "./HarnessLogo";
import { Md } from "./Md";

type Props = {
  part: ChatPart & { orx: OrxSource };
  runExperimentName?: (runId: string) => string;
  onOpenFile?: ComponentProps<typeof Md>["onOpenFile"];
  onOpenRun?: ComponentProps<typeof Md>["onOpenRun"];
};

/** A message orx or another chat's agent wrote, on the left under its sender. */
export function AgentMessageCard(props: Props) {
  return props.part.orx.kind === "run" ? <RunCard {...props} /> : <SentCard {...props} />;
}

/** A run that ended: its experiment, outcome and how long it ran, opening its logs. */
function RunCard({ part, runExperimentName, onOpenRun }: Props) {
  const { from: runId, status } = part.orx;
  const scope = useContext(FileScopeContext);
  const { listRunsQuery } = useWorkspace().queries;
  const { data: run } = useQuery({
    ...listRunsQuery(scope?.projectId ?? ""),
    enabled: Boolean(scope && runId),
    select: (runs) => runs.find((candidate) => candidate.id === runId),
  });
  const took = run?.endedAt ? fmtDuration(run.endedAt - run.createdAt) : null;
  const [Icon, label, tone] = status === "done"
    ? [CircleCheck, took ? m.run_done_in({ duration: took }) : m.status_done(), "bg-accent-green-subtle [&_.tone]:text-accent-green"]
    : status === "failed"
      ? [CircleX, took ? m.run_failed_after({ duration: took }) : m.status_failed(), "bg-accent-red-subtle [&_.tone]:text-accent-red"]
      : [CircleAlert, m.status_lost(), "bg-accent-amber-subtle [&_.tone]:text-accent-amber"];
  const name = (runId && runExperimentName?.(runId)) || m.tree_experiment();
  return (
    <button
      type="button"
      className={`agent-message me-auto flex w-fit min-w-0 max-w-[88%] items-center gap-2 rounded-full py-1.5 ps-3 pe-3.5 text-start text-base enabled:cursor-pointer enabled:hover:brightness-110 focus-visible:outline-2 focus-visible:outline-focus ${tone}`}
      disabled={!runId || !onOpenRun}
      title={runId ? m.a11y_open_logs_for_run({ run: runId }) : undefined}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => runId && onOpenRun?.(runId, intent))}
    >
      <Icon size={16} aria-hidden="true" className="tone shrink-0" />
      <span className="min-w-0 truncate text-text">{name}</span>
      <span className="tone shrink-0">{label}</span>
    </button>
  );
}

/** A task, message, answer or question from another chat, or orx's warning
 * about runs: its sender opens that chat, the rest opens the whole message. */
function SentCard({ part, onOpenFile, onOpenRun }: Props) {
  const [open, setOpen] = useState(false);
  const links = useContext(ChatLinksContext);
  const { kind, from } = part.orx;
  const text = part.text ?? "";
  const chat = kind === "monitoring" || !from ? undefined : links?.chat(from);
  const sender = kind === "monitoring"
    ? m.agent_message_openresearch()
    : chat?.title || m.agent_message_other_chat();
  const icon = kind === "monitoring"
    ? <TriangleAlert size={16} aria-hidden="true" className="shrink-0 text-accent-amber" />
    : chat
      ? <HarnessLogo harness={chat.harness} size={16} />
      : <MessagesSquare size={16} aria-hidden="true" className="shrink-0" />;
  const preview = kind === "monitoring" ? m.agent_message_runs_unmonitored() : firstLine(text);
  const senderClass = "flex min-w-0 max-w-[45%] shrink-0 items-center gap-2 py-1.5 ps-3 pe-1 text-subtext [&_svg]:text-subtext";
  return (
    <div className="agent-message me-auto flex w-fit min-w-0 max-w-[88%] flex-col items-start gap-1.5">
      <div className={`flex min-w-0 max-w-full items-center rounded-full ${open ? "" : "bg-panel has-[>button:last-child:hover]:bg-surface"}`}>
        {chat && from ? (
          <button
            type="button"
            className={`${senderClass} cursor-pointer hover:text-text hover:underline focus-visible:outline-2 focus-visible:outline-focus`}
            onClick={() => links?.onOpenChat(from)}
          >
            {icon}
            <span className="min-w-0 truncate">{sender}</span>
          </button>
        ) : (
          <span className={senderClass}>
            {icon}
            <span className="min-w-0 truncate">{sender}</span>
          </span>
        )}
        <button
          type="button"
          aria-expanded={open}
          aria-label={open ? undefined : preview}
          className="flex min-w-0 cursor-pointer items-center gap-2 rounded-e-full py-1.5 ps-1 pe-2.5 text-start text-base focus-visible:outline-2 focus-visible:outline-focus"
          onClick={() => setOpen((value) => !value)}
        >
          {!open && preview && <span className="min-w-0 truncate text-text"><PreviewText text={preview} /></span>}
          <ChevronDown
            size={14}
            aria-hidden="true"
            className={`shrink-0 text-muted transition-transform duration-200 ease-standard motion-reduce:transition-none ${open ? "rotate-180" : ""}`}
          />
        </button>
      </div>
      {open && (
        <div dir="auto" className="msg-agent max-w-full rounded-2xl bg-panel px-4 py-2.5 text-base wrap-anywhere">
          <Md text={text} onOpenFile={onOpenFile} onOpenRun={onOpenRun} />
        </div>
      )}
    </div>
  );
}

/** A preview line with its cited messages' first words and chats by name. */
function PreviewText({ text }: { text: string }) {
  const links = useContext(ChatLinksContext);
  const pieces = [];
  let last = 0;
  for (const match of text.matchAll(CITED_ID)) {
    pieces.push(text.slice(last, match.index));
    pieces.push(match[0].startsWith("msg_")
      ? <CitedWords key={match.index} cited={match[0]} />
      : links?.chat(match[0])?.title || match[0]);
    last = match.index + match[0].length;
  }
  pieces.push(text.slice(last));
  return <>{pieces}</>;
}
