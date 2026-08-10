import type { SkillInfo } from "../api";

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
  return (
    <div className="skill-menu absolute bottom-[calc(100%_+_8px)] left-0 min-w-85 max-w-full p-1.5 bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden">
      {skills.map((s, i) => (
        <button
          key={s.name}
          type="button"
          className={`skill-item flex flex-col gap-0.5 w-full text-left py-[7px] px-2 rounded-sm [&.active]:bg-surface [&_.skill-name]:text-md [&_.skill-hint]:text-muted [&_.skill-desc]:text-sm [&_.skill-desc]:text-subtext ${i === activeIndex ? "active" : ""}`}
          // mousedown + preventDefault keeps the textarea focused.
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(s);
          }}
          onMouseEnter={() => onHover(i)}
        >
          <span className="skill-name">
            /{s.name} <span className="skill-hint">{s.argHint}</span>
          </span>
          <span className="skill-desc">{s.description}</span>
        </button>
      ))}
    </div>
  );
}
