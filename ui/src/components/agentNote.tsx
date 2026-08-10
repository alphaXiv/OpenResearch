import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { MONO_CLASS_NAME } from "../styleClasses";

/** A backtick command from an `agentNote`, rendered as a code pill with its own
 * copy button so the user can grab it without retyping. */
function CommandPill({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="cmd-inline [display:inline-flex] [align-items:center] [gap:4px] [vertical-align:baseline]">
      <code className={MONO_CLASS_NAME}>{cmd}</code>
      <button
        type="button"
        className="cmd-inline-copy [display:inline-flex] [align-items:center] [padding:2px] [border:0] [border-radius:var(--radius-xs)] [background:none] [color:var(--muted)] [cursor:pointer] [&:hover]:[background:var(--surface)] [&:hover]:[color:var(--text)]"
        onClick={() => {
          void navigator.clipboard
            .writeText(cmd)
            .then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
            .catch(() => {});
        }}
        aria-label={copied ? "Copied" : `Copy ${cmd}`}
        title={copied ? "Copied" : "Copy"}
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
