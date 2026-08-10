import { GitBranch, RotateCw } from "lucide-react";
import { GitHubMark } from "./BackendLogos";
import { ICON_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

export type CodeBrowserView = "files" | "changes";

export function CodeBrowserHeader({
  view,
  onViewChange,
  showViewToggle = true,
  branchLabel,
  branchTitle,
  githubHref,
  githubTitle,
  refreshing,
  onRefresh,
}: {
  view: CodeBrowserView;
  onViewChange: (view: CodeBrowserView) => void;
  showViewToggle?: boolean;
  branchLabel?: string;
  branchTitle?: string;
  githubHref?: string;
  githubTitle?: string;
  refreshing: boolean;
  onRefresh: () => void;
}) {
  return (
    <div className="code-tab-header [display:flex] [align-items:center] [gap:8px] [padding:6px_12px] [border-bottom:1px_solid_var(--border-variant)] [flex-shrink:0] [&_>_.seg]:[padding:2px] [&_>_.seg]:[border-radius:var(--radius-sm)] [&_>_.seg_button]:[padding:2px_8px] [&_>_.seg_button]:[font-size:var(--fs-sm)] [&_>_.seg_button]:[font-weight:var(--fw-medium)]">
      {showViewToggle && (
        <div className="seg [display:inline-flex] [align-items:center] [gap:2px] [padding:3px] [border-radius:var(--radius-md)] [background:color-mix(in_oklab,_var(--text)_10%,_transparent)] [&_button]:[padding:3px_12px] [&_button]:[font-size:var(--fs-md)] [&_button]:[font-weight:var(--fw-semibold)] [&_button]:[color:var(--text)] [&_button]:[border-radius:var(--radius-sm)] [&_button:not(:disabled):hover]:[color:var(--text)] [&_button.active]:[background:var(--base)] [&_button.active]:[box-shadow:0_1px_3px_color-mix(in_oklab,_var(--text)_25%,_transparent)] [&_button:disabled]:[color:var(--muted)] [&_button:disabled]:[cursor:default]" role="group" aria-label="Code browser view">
          <button
            type="button"
            className={view === "files" ? "active" : ""}
            aria-pressed={view === "files"}
            onClick={() => onViewChange("files")}
          >
            Files
          </button>
          <button
            type="button"
            className={view === "changes" ? "active" : ""}
            aria-pressed={view === "changes"}
            onClick={() => onViewChange("changes")}
          >
            Changes
          </button>
        </div>
      )}
      {branchLabel && (
        <span className="wt-branch-chip [display:inline-flex] [align-items:center] [gap:4px] [min-width:0] [padding:2px_8px] [border-radius:var(--radius-full)] [background:color-mix(in_oklab,_var(--text)_8%,_transparent)] [color:var(--subtext)] [font-size:var(--fs-xs)] [&_>_svg]:[flex-shrink:0]" title={branchTitle}>
          <GitBranch size={12} />
          <span className="wt-branch-name [overflow:hidden] [text-overflow:ellipsis] [white-space:nowrap] [font-family:var(--mono)]">{branchLabel}</span>
        </span>
      )}
      {githubHref && (
        <a
          className={ICON_BUTTON_CLASS_NAME}
          href={githubHref}
          target="_blank"
          rel="noopener noreferrer"
          title={githubTitle}
          aria-label={githubTitle}
        >
          <GitHubMark size={13} />
        </a>
      )}
      <span style={{ flex: 1 }} />
      <button className={ICON_BUTTON_CLASS_NAME} title="Refresh" aria-label="Refresh" onClick={onRefresh}>
        {refreshing ? <span className={SPINNER_CLASS_NAME} /> : <RotateCw size={13} />}
      </button>
    </div>
  );
}
