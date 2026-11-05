import { useLayoutEffect, useRef } from "react";
import { Folder } from "lucide-react";
import type { AlphaXivPaper, HarnessId } from "../api";
import { splitMentionPath, type FileMentionMatch } from "../fileMentions";
import { m } from "../paraglide/messages.js";
import { FileTypeIcon } from "./FileTypeIcon";
import { HarnessLogo } from "./HarnessLogo";
import { LitSourceLogo } from "./LitSourceLogo";

/** Something `@` adds to the draft as context. */
export type MentionMatch =
  | ({ kind: "file" } & FileMentionMatch)
  | { kind: "paper"; paper: AlphaXivPaper }
  | { kind: "chat"; id: string; title: string; harness: HarnessId };

const SECTION_LABEL: Record<MentionMatch["kind"], () => string> = {
  file: m.mention_menu_files,
  paper: m.mention_menu_papers,
  chat: m.mention_menu_chats,
};

function MentionRow({ match }: { match: MentionMatch }) {
  if (match.kind === "paper") {
    return (
      <>
        <LitSourceLogo source="alphaxiv" decorative />
        <span className="min-w-0 truncate">{match.paper.title}</span>
        {match.paper.authors[0] && <span className="shrink-0 text-muted">{match.paper.authors[0]}</span>}
      </>
    );
  }
  if (match.kind === "chat") {
    return (
      <>
        <HarnessLogo harness={match.harness} size={16} />
        <span className="min-w-0 truncate">{match.title}</span>
      </>
    );
  }
  const { name, parent } = splitMentionPath(match.path);
  return (
    <>
      {match.directory ? <Folder size={16} className="shrink-0 text-muted" /> : <FileTypeIcon name={name} />}
      <span className="shrink-0" dir="ltr">{name}</span>
      {parent && <span className="min-w-0 truncate text-muted" dir="ltr">{parent}</span>}
    </>
  );
}

/** `@` dropdown above the composer: files, papers and chats. Like SkillMenu, ChatPanel owns its state. */
export function MentionMenu({
  matches,
  activeIndex,
  onPick,
  onHover,
}: {
  matches: MentionMatch[];
  activeIndex: number;
  onPick: (match: MentionMatch) => void;
  onHover: (index: number) => void;
}) {
  const activeRef = useRef<HTMLButtonElement>(null);
  const sectioned = new Set(matches.map((match) => match.kind)).size > 1;

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, matches]);

  return (
    <div className="file-mention-menu absolute bottom-[calc(100%_+_8px)] start-0 w-full max-h-[min(24rem,50vh)] overflow-y-auto overscroll-contain p-1.5 bg-background border border-border-variant rounded-lg shadow-control-subtle z-50">
      {matches.map((match, i) => (
        <div key={match.kind === "file" ? match.path : match.kind === "paper" ? match.paper.url : match.id}>
          {sectioned && match.kind !== matches[i - 1]?.kind && (
            <div className="px-2 pt-1.5 pb-1 text-xs text-muted">{SECTION_LABEL[match.kind]()}</div>
          )}
          <button
            ref={i === activeIndex ? activeRef : undefined}
            type="button"
            className={`file-mention-item flex items-center gap-2 w-full text-start py-1 px-2 rounded-sm text-sm font-normal text-text/80 [&.active]:bg-hover-muted [&.active]:text-text ${i === activeIndex ? "active" : ""}`}
            // mousedown + preventDefault keeps the editor focused.
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(match);
            }}
            onMouseEnter={() => onHover(i)}
          >
            <MentionRow match={match} />
          </button>
        </div>
      ))}
    </div>
  );
}
