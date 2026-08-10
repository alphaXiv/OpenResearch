import { githubBranchUrl } from "../api";
import { GitHubMark } from "./BackendLogos";

const FILES_PILL_CLASS_NAME = [
  "files-pill [display:inline-flex] [align-items:center] [gap:8px] [min-width:0] [border:1px_solid_var(--border)]",
  "[border-radius:var(--radius-md)] [padding:7px_11px] [background:var(--base)] [color:var(--text)]",
  "[text-decoration:none] [&_code]:[font-family:var(--mono)] [&_code]:[font-size:var(--fs-sm)]",
  "[&_code]:[overflow:hidden] [&_code]:[text-overflow:ellipsis] [&_code]:[white-space:nowrap]",
  "[&_>_svg]:[flex-shrink:0] [&_>_svg]:[color:var(--muted)] [a&:hover]:[border-color:var(--muted)]",
].join(" ");

/** A branch name as a pill linking to its GitHub tree view (files-pill
 * styling). */
export function BranchPill({
  owner,
  repo,
  branch,
}: {
  owner: string;
  repo: string;
  branch: string;
}) {
  if (!owner || !repo) {
    return <span className={FILES_PILL_CLASS_NAME}><code>{branch}</code></span>;
  }
  return (
    <a
      className={FILES_PILL_CLASS_NAME}
      href={githubBranchUrl(owner, repo, branch)}
      target="_blank"
      rel="noopener noreferrer"
      title={`Open ${branch} on GitHub`}
    >
      <code>{branch}</code>
      <GitHubMark size={12} />
    </a>
  );
}
