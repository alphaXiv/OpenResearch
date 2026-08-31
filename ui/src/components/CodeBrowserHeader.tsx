import { m } from "../paraglide/messages.js";
import { GitBranch, RotateCw } from "lucide-react";
import { GitHubMark } from "./BackendLogos";
import { IconButton, IconButtonLink, Spinner } from "./ui";

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
    <div className="code-tab-header flex items-center gap-2 py-1.5 px-3 border-b border-b-border-variant shrink-0 [&_>_.seg]:p-0.5 [&_>_.seg]:rounded-sm [&_>_.seg_button]:py-0.5 [&_>_.seg_button]:px-2 [&_>_.seg_button]:text-sm [&_>_.seg_button]:font-medium">
      {showViewToggle && (
        <div className="seg inline-flex items-center gap-0.5 p-[3px] rounded-md bg-hover-subtle [&_button]:py-[3px] [&_button]:px-3 [&_button]:text-sm [&_button]:font-medium [&_button]:text-text [&_button]:rounded-sm [&_button:not(:disabled):hover]:text-text [&_button.active]:bg-background [&_button.active]:shadow-segment [&_button:disabled]:text-muted [&_button:disabled]:cursor-default" role="group" aria-label={m.code_browser_header_code_browser_view()}>
          <button
            type="button"
            className={view === "files" ? "active" : ""}
            aria-pressed={view === "files"}
            onClick={() => onViewChange("files")}
          >
            {m.code_browser_header_files()}
          </button>
          <button
            type="button"
            className={view === "changes" ? "active" : ""}
            aria-pressed={view === "changes"}
            onClick={() => onViewChange("changes")}
          >
            {m.code_browser_header_changes()}
          </button>
        </div>
      )}
      {branchLabel && (
        <span className="wt-branch-chip inline-flex items-center gap-1 min-w-0 py-0.5 px-2 rounded-full bg-hover-muted text-subtext text-xs [&_>_svg]:shrink-0" title={branchTitle}>
          <GitBranch size={12} />
          <span className="wt-branch-name overflow-hidden text-ellipsis whitespace-nowrap">{branchLabel}</span>
        </span>
      )}
      {githubHref && (
        <IconButtonLink
          href={githubHref}
          target="_blank"
          rel="noopener noreferrer"
          title={githubTitle}
          aria-label={githubTitle}
        >
          <GitHubMark size={13} />
        </IconButtonLink>
      )}
      <span className="flex-1" />
      <IconButton title={m.code_browser_header_refresh()} aria-label={m.code_browser_header_refresh()} onClick={onRefresh}>
        {refreshing ? <Spinner /> : <RotateCw size={13} />}
      </IconButton>
    </div>
  );
}
