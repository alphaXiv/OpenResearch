import { githubBranchUrl } from "../api";
import { GitHubMark } from "./BackendLogos";

const FILES_PILL_CLASS_NAME = [
  "files-pill inline-flex items-center gap-2 min-w-0 border border-border",
  "rounded-md py-[7px] px-[11px] bg-background text-text",
  "no-underline [&_code]:font-mono [&_code]:text-sm",
  "[&_code]:overflow-hidden [&_code]:text-ellipsis [&_code]:whitespace-nowrap",
  "[&_>_svg]:shrink-0 [&_>_svg]:text-muted [a&:hover]:border-muted",
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
