import { ListChecks, WandSparkles } from "lucide-react";
import { useLayoutEffect, useRef } from "react";
import type { SkillInfo } from "../api";
import { commandLabel } from "../planCommand";
import { m } from "../paraglide/messages.js";

/** Slash-skill dropdown above the composer. Open/filter/keyboard state lives
 * in ChatPanel (it's derived from the draft); this just renders the matches. */
export function SkillMenu({
  skills,
  activeIndex,
  onPick,
  onHover,
}: {
  skills: SkillInfo[];
  activeIndex: number;
  onPick: (skill: SkillInfo) => void;
  onHover: (index: number) => void;
}) {
  const activeRef = useRef<HTMLButtonElement>(null);

  useLayoutEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, skills]);

  return (
    <div className="skill-menu absolute bottom-[calc(100%_+_8px)] start-0 w-full max-h-[min(18rem,40vh)] overflow-y-auto overscroll-contain p-1.5 bg-background border border-border-variant rounded-2xl shadow-control-subtle z-50">
      {skills.map((s, i) => (
        <button
          key={s.name}
          ref={i === activeIndex ? activeRef : undefined}
          type="button"
          className={`skill-item flex items-center gap-2 w-full text-start py-1 px-2 rounded-full text-sm font-normal text-text/80 [&.active]:bg-hover-muted [&.active]:text-text ${i === activeIndex ? "active" : ""}`}
          // mousedown + preventDefault keeps the textarea focused.
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(s);
          }}
          onMouseEnter={() => onHover(i)}
        >
          {s.source === "command" && s.name === "plan" ? (
            <ListChecks size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />
          ) : (
            <WandSparkles size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />
          )}
          <span className="skill-name shrink-0">
            {commandLabel(s)}
          </span>
          <span className="skill-desc min-w-0 truncate text-muted">{s.description}</span>
          {s.source === "user" && (
            <span className="ms-auto shrink-0 ps-2 text-muted">{m.skill_menu_personal()}</span>
          )}
        </button>
      ))}
    </div>
  );
}
