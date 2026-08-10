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
    <div className="skill-menu [position:absolute] [bottom:calc(100%_+_8px)] [left:0] [min-width:340px] [max-width:100%] [padding:6px] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [z-index:50] [overflow:hidden]">
      {skills.map((s, i) => (
        <button
          key={s.name}
          type="button"
          className={`skill-item [display:flex] [flex-direction:column] [gap:2px] [width:100%] [text-align:left] [padding:7px_8px] [border-radius:var(--radius-sm)] [&.active]:[background:var(--surface)] [&_.skill-name]:[font-size:var(--fs-md)] [&_.skill-hint]:[color:var(--muted)] [&_.skill-desc]:[font-size:var(--fs-sm)] [&_.skill-desc]:[color:var(--subtext)] ${i === activeIndex ? "active" : ""}`}
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
