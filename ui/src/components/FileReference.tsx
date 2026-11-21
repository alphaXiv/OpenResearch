import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, FileCode } from "lucide-react";
import { createContext, useContext, useMemo, type ReactNode } from "react";
import { ltr } from "../i18n";
import { ARTIFACTS_PREFIX, artifactFiles, GLOB_CHARS, resolveArtifactReference, resolveFileReference, type FileResolution } from "../fileReferences";
import { splitMentionPath } from "../fileMentions";
import { isWindowsDrivePath, splitLineSuffix } from "../markdownTarget";
import { m } from "../paraglide/messages.js";
import type { ProjectArtifacts } from "../api";

import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { MenuItem } from "./ui";

export type OpenFile = (
  path: string,
  line: number | undefined,
  exp: string | undefined,
  ref: string | undefined,
  intent: TabOpenIntent,
) => void;

/** The checkout a chat's file references are checked against. */
export const FileScopeContext = createContext<{ projectId: string; sessionId?: string | null } | undefined>(undefined);

export function FileChip({ path, line, exp, onOpenFile }: {
  path: string;
  line?: number;
  /** Experiment id this file was cited from, if any (`<file exp=…>`). */
  exp?: string;
  onOpenFile?: OpenFile;
}) {
  const name = path.split("/").pop() || path;
  const label = line != null ? `${name}:${line}` : name;
  return (
    <button
      className="file-chip"
      title={onOpenFile ? m.a11y_open_file_in_panel({ path: ltr(path) }) : path}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
        onOpenFile?.(path, line, exp, undefined, intent),
      )}
      disabled={!onOpenFile}
    >
      <FileCode size={12} />
      <span className="file-chip-label">{label}</span>
    </button>
  );
}

/** A reference that is not certain to be one file: the reader picks the match. */
function FileChoices({ label, paths, total, line, onOpenFile }: {
  label: string;
  paths: string[];
  total: number;
  line?: number;
  onOpenFile: OpenFile;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const title = m.md_pick_file({ path: ltr(label) });
  return (
    <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root modal={false}>
        <Menu.Trigger className="file-chip" title={title} aria-label={title}>
          <FileCode size={12} />
          <span className="file-chip-label">{label}</span>
          <ChevronDown size={12} />
        </Menu.Trigger>
        <Menu.Portal container={portalContainer}>
          <Menu.Positioner className="z-50" positionMethod="fixed" align="start" sideOffset={4} collisionPadding={8}>
            <Menu.Popup aria-label={title} className="outline-none max-h-(--available-height) overflow-y-auto min-w-48 max-w-[min(32rem,calc(100vw-2rem))] rounded-lg border border-border bg-background p-1.5 shadow-menu">
              {paths.map((path) => {
                const { name, parent } = splitMentionPath(path);
                return (
                  <Menu.Item key={path} nativeButton render={<MenuItem size="compact" className="text-sm data-highlighted:bg-surface" />}
                    onClick={() => onOpenFile(path, line, undefined, undefined, "preview")}>
                    <span dir="ltr" className="min-w-0 truncate">
                      <span className="font-medium">{name}</span>
                      {parent && <span className="ms-2 text-subtext">{parent}</span>}
                    </span>
                  </Menu.Item>
                );
              })}
              {total > paths.length && (
                <div className="px-2 py-1 text-xs text-subtext">{m.common_more_count({ count: total - paths.length })}</div>
              )}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
  );
}

const isProjectRelative = (path: string) =>
  !path.startsWith("/") && !path.startsWith("~") && !isWindowsDrivePath(path);

const NO_MATCH: FileResolution = { kind: "none" };

const artifactListing = (data: ProjectArtifacts) => ({ files: artifactFiles(data.entries), truncated: data.truncated });

/**
 * A reference to a project file, checked against the chat's checkout and the
 * project's artifacts so it never looks clickable and fails: an exact path
 * opens, a fuzzy or glob match asks which file, and a path that names nothing
 * renders as `fallback`.
 * The checkout listing omits gitignored files and caps out on huge repos, so a
 * path it lacks is read directly before the artifacts are searched; an
 * `artifacts/` citation skips that listing and prefers an exact artifact.
 * An `optimistic` citation (a `<file>` tag or link) shows as a chip until
 * verification says otherwise; inferred ones (code spans) stay `fallback` until verified.
 */
export function FileReference({ path, line, exp, optimistic, fallback, onOpenFile }: {
  path: string;
  line?: number;
  exp?: string;
  optimistic: boolean;
  fallback: ReactNode;
  onOpenFile: OpenFile;
}) {
  const { getCodeTreeQuery, getProjectFileQuery, getArtifactsQuery } = useWorkspace().queries;

  const scope = useContext(FileScopeContext);
  // An experiment's file lives on its own branch, which this listing does not cover.
  const checkable = scope != null && exp == null && isProjectRelative(path);
  const citesArtifact = ARTIFACTS_PREFIX.test(path);
  const tree = useQuery({
    ...getCodeTreeQuery(scope?.projectId ?? "", scope?.sessionId ? { sessionId: scope.sessionId } : {}),
    // The agent cites files it just wrote; a long-cached listing would miss them.
    staleTime: 5_000,
    enabled: checkable && !citesArtifact,
  });
  const entries = tree.data?.entries;
  const resolution = useMemo(
    () => (citesArtifact ? NO_MATCH : entries ? resolveFileReference(entries, path) : null),
    [citesArtifact, entries, path],
  );
  const unlisted = resolution?.kind === "none" && !GLOB_CHARS.test(path);
  // An `artifacts/` citation may name a checkout folder of that name, which the viewer also opens.
  const read = useQuery({
    ...getProjectFileQuery(scope?.projectId ?? "", splitLineSuffix(path).path, scope?.sessionId ? { sessionId: scope.sessionId } : {}),
    enabled: checkable && unlisted,
  });
  const onDisk = read.isSuccess && !read.data.notFound;
  const searchArtifacts = checkable && (citesArtifact || (unlisted && read.isSuccess && read.data.notFound));
  const artifacts = useQuery({
    ...getArtifactsQuery(scope?.projectId ?? ""),
    staleTime: 5_000,
    select: artifactListing,
    enabled: searchArtifacts,
  });
  const files = artifacts.data?.files;
  const artifactMatch = useMemo(() => (files ? resolveArtifactReference(files, path) : null), [files, path]);
  const chip = <FileChip path={citesArtifact ? path.replace(ARTIFACTS_PREFIX, "artifacts/") : path} line={line} exp={exp} onOpenFile={onOpenFile} />;

  if (!checkable || !resolution) return optimistic ? chip : fallback;
  const match = resolution.kind !== "none" ? resolution
    : citesArtifact && artifactMatch?.kind === "exact" ? artifactMatch
    : onDisk ? null
    : artifactMatch;
  if (match?.kind === "exact") return <FileChip path={match.path} line={match.line ?? line} exp={exp} onOpenFile={onOpenFile} />;
  if (match?.kind === "choices") {
    return <FileChoices label={path} paths={match.paths} total={match.total} line={match.line ?? line} onOpenFile={onOpenFile} />;
  }
  if (onDisk) return chip;
  // A truncated or failed artifacts listing can't rule the file out.
  const unsure = (unlisted && read.isPending) ||
    (searchArtifacts && (artifacts.isPending || artifacts.isError || artifacts.data?.truncated === true));
  return optimistic && unsure ? chip : fallback;
}
