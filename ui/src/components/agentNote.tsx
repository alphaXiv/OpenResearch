import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";

/** A backtick command from an `agentNote`, rendered as a code pill with its own
 * copy button so the user can grab it without retyping. */
function CommandPill({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="cmd-inline inline-flex items-center gap-1 align-baseline">
      <code className="font-mono text-sm">{cmd}</code>
      <button
        type="button"
        className="cmd-inline-copy inline-flex items-center p-0.5 border-0 rounded-xs bg-none bg-transparent text-muted cursor-pointer [&:hover]:bg-surface [&:hover]:text-text"
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
 * onboarding, the model picker, and the chat panel. */
export function renderNote(note: string | undefined) {
  if (!note) return null;
  return note
    .split(/`([^`]+)`/)
    .map((part, i) => (i % 2 === 1 ? <CommandPill key={i} cmd={part} /> : part));
}
