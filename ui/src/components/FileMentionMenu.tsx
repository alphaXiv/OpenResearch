import { useLayoutEffect, useRef } from "react";
import { Folder } from "lucide-react";
import { splitMentionPath, type FileMentionMatch } from "../fileMentions";
import { FileTypeIcon } from "./FileTypeIcon";

/** `@file` dropdown above the composer; like SkillMenu, ChatPanel owns its state. */
export function FileMentionMenu({
  matches,
  activeIndex,
  onPick,
  onHover,
}: {
  matches: FileMentionMatch[];
  activeIndex: number;
  onPick: (match: FileMentionMatch) => void;
  onHover: (index: number) => void;
}) {
  const activeRef = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, matches]);

  return (
    <div className="file-mention-menu absolute bottom-[calc(100%_+_8px)] start-0 w-full max-h-[min(18rem,40vh)] overflow-y-auto overscroll-contain p-1.5 bg-background border border-border-variant rounded-2xl shadow-control-subtle z-50">
      {matches.map((match, i) => {
        const { name, parent } = splitMentionPath(match.path);
        return (
          <button
            key={match.path}
            ref={i === activeIndex ? activeRef : undefined}
            type="button"
            className={`file-mention-item flex items-center gap-2 w-full text-start py-1 px-2 rounded-full text-sm font-normal text-text/80 [&.active]:bg-hover-muted [&.active]:text-text ${i === activeIndex ? "active" : ""}`}
            // mousedown + preventDefault keeps the textarea focused.
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(match);
            }}
            onMouseEnter={() => onHover(i)}
          >
            {match.directory ? <Folder size={15} className="shrink-0 text-muted" /> : <FileTypeIcon name={name} />}
            <span className="shrink-0" dir="ltr">{name}</span>
            {parent && <span className="min-w-0 truncate text-muted" dir="ltr">{parent}</span>}
          </button>
        );
      })}
    </div>
  );
}
