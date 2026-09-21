import { FileCode } from "lucide-react";
import { useLayoutEffect, useRef } from "react";
import { mentionBasename } from "../mentionCommand";
import { m } from "../paraglide/messages.js";

/** `@`-mention dropdown above the composer, mirroring `SkillMenu`. Open/
 * filter/keyboard state lives in ChatPanel (derived from the draft and a
 * `getCodeTreeQuery` listing); this just renders the ranked matches. */
export function MentionMenu({
  paths,
  activeIndex,
  onPick,
  onHover,
}: {
  paths: string[];
  activeIndex: number;
  onPick: (path: string) => void;
  onHover: (index: number) => void;
}) {
  const activeRef = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, paths]);

  return (
    <div className="mention-menu absolute bottom-[calc(100%_+_8px)] start-0 w-full max-h-[min(18rem,40vh)] overflow-y-auto overscroll-contain p-1.5 bg-background border border-border-variant rounded-2xl shadow-control-subtle z-50">
      {paths.length === 0 ? (
        <div className="py-1 px-2 text-sm text-muted">{m.mention_menu_no_matches()}</div>
      ) : (
        paths.map((path, i) => (
          <button
            key={path}
            ref={i === activeIndex ? activeRef : undefined}
            type="button"
            className={`mention-item flex items-center gap-2 w-full text-start py-1 px-2 rounded-full text-sm font-normal text-text/80 [&.active]:bg-hover-muted [&.active]:text-text ${i === activeIndex ? "active" : ""}`}
            // mousedown + preventDefault keeps the textarea focused.
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(path);
            }}
            onMouseEnter={() => onHover(i)}
          >
            <FileCode size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />
            <span className="mention-name shrink-0 max-w-[45%] truncate">
              {mentionBasename(path)}
            </span>
            <span className="mention-path min-w-0 truncate text-muted">{path}</span>
          </button>
        ))
      )}
    </div>
  );
}
