import { Hourglass } from "lucide-react";
import { useEffect, useState } from "react";
import { fmtDuration, fmtNumber, type ChatSession } from "../api";
import { m } from "../paraglide/messages.js";

/** Tasks the agent left running when its turn ended; it picks the chat back up when they finish. */
export function BackgroundTaskBanner({ background }: { background: ChatSession["background"] }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  if (!background) return null;
  return (
    <div className="mb-2.5 flex min-w-0 items-center gap-2 rounded-lg border border-border bg-accent-blue-subtle px-3 py-2 text-sm text-text">
      <Hourglass size={16} className="shrink-0 text-accent-blue" aria-hidden="true" />
      <span role="status" className="shrink-0 font-medium">
        {background.count > 1 ? m.chat_status_background_tasks({ count: fmtNumber(background.count) }) : m.chat_status_background_task()}
      </span>
      <span aria-hidden="true">·</span>
      <span className="min-w-0 truncate text-subtext">{m.chat_background_resumes()}</span>
      <span className="ms-auto shrink-0 tabular-nums text-subtext">{fmtDuration(now - background.since)}</span>
    </div>
  );
}
