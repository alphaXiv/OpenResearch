import { useId } from "react";
import { useQuery } from "@tanstack/react-query";
import { NotebookPen } from "lucide-react";
import type { ChatPart, PromptAnswer } from "../api";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { GitDiff } from "./GitDiff";
import { Button } from "./ui";

/** The card `orx instructions propose` shows: the change as a diff, applied only once the user approves. */
export function InstructionsProposalCard({
  part,
  done,
  respond,
}: {
  part: ChatPart;
  done: boolean;
  respond: (answer: Omit<PromptAnswer, "promptId">) => Promise<unknown>;
}) {
  const prompt = part.prompt!;
  const headingId = useId();
  const saved = useQuery({ ...useWorkspace().queries.getInstructionsQuery(), enabled: !prompt.resolved }).data;
  if (prompt.resolved) {
    return (
      <div className="flex items-center gap-2 py-1 text-base text-subtext">
        <NotebookPen size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />
        {prompt.approved ? m.chat_instructions_approved() : m.chat_instructions_declined()}
      </div>
    );
  }
  const stale = saved !== undefined && saved.version !== prompt.instructionsVersion;
  return (
    <section
      className="prompt-card instructions my-3 w-full max-w-2xl overflow-hidden rounded-lg border border-border bg-background"
      aria-labelledby={headingId}
    >
      <div className="flex items-center gap-2.5 px-3.5 pt-3">
        <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-surface text-subtext">
          <NotebookPen size={16} strokeWidth={1.5} aria-hidden="true" />
        </span>
        <span id={headingId} className="text-base font-semibold text-text">{m.chat_instructions_title()}</span>
      </div>
      <div className="max-h-96 overflow-auto px-3.5 py-3">
        <GitDiff diff={prompt.diff ?? ""} />
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border-variant px-3.5 py-2.5">
        <p role={stale ? "alert" : undefined} className={`m-0 me-auto min-w-0 text-sm leading-6 ${stale ? "text-accent-amber" : "text-subtext"}`}>
          {stale ? m.chat_instructions_stale() : m.chat_instructions_note()}
        </p>
        {!done && (
          <div className="flex gap-2">
            <Button type="button" size="small" variant="ghost" onClick={() => void respond({ approve: false })}>
              {m.chat_instructions_decline()}
            </Button>
            {!stale && (
              <Button type="button" size="small" variant="primary" onClick={() => void respond({ approve: true })}>
                {m.chat_instructions_approve()}
              </Button>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
