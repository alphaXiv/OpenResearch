import { useState } from "react";
import { Check, Copy, Play } from "lucide-react";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";

/** A backtick command from an `agentNote`, rendered as a code pill with its own
 * copy button so the user can grab it without retyping. */
const PILL_BUTTON_CLASS_NAME = "inline-flex items-center p-0.5 border-0 rounded-xs bg-none bg-transparent text-muted cursor-pointer [&:hover]:bg-surface [&:hover]:text-text";

function CommandPill({ cmd, onRun }: { cmd: string; onRun?: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="cmd-inline inline-flex items-center gap-1 align-baseline">
      <code dir="ltr" className="font-mono text-sm">{cmd}</code>
      {onRun && (
        <button
          type="button"
          className={`cmd-inline-run ${PILL_BUTTON_CLASS_NAME}`}
          onClick={onRun}
          aria-label={m.a11y_run_command({ value: ltr(cmd) })}
          title={m.a11y_run_command({ value: ltr(cmd) })}
        >
          <Play size={11} />
        </button>
      )}
      <button
        type="button"
        className={`cmd-inline-copy ${PILL_BUTTON_CLASS_NAME}`}
        onClick={() => {
          void navigator.clipboard
            .writeText(cmd)
            .then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
            .catch(() => {});
        }}
        aria-label={copied ? m.common_copied() : m.a11y_copy_value({ value: ltr(cmd) })}
        title={copied ? m.common_copied() : m.md_copy()}
      >
        {copied ? <Check size={11} strokeWidth={3} /> : <Copy size={11} />}
      </button>
    </span>
  );
}

/** Harness `agentNote` strings carry the command to run in backticks
 * (`claude auth login`) — render those spans as copyable code pills so they read
 * as something to type, not prose. Shared by every surface that shows a note:
 * onboarding, the model picker, and the chat panel. Commands `runnable` accepts
 * also get a play button that runs them in place. */
export function renderNote(
  note: string | undefined,
  runnable?: { canRun: (command: string) => boolean; onRun: (command: string) => void },
) {
  if (!note) return null;
  return note.split(/`([^`]+)`/).map((part, i) =>
    i % 2 === 1 ? (
      <CommandPill key={i} cmd={part} onRun={runnable?.canRun(part) ? () => runnable.onRun(part) : undefined} />
    ) : (
      part
    ),
  );
}
