import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MessageCircleDashed, Search } from "lucide-react";
import type { AgentIconColor, Project, ProjectIcon } from "../api";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { AgentIconTile, IconPicker, IconPickerButton, TILE_SIZES, type Logo } from "./AgentIcon";
import { Button, Input, Spinner, Tooltip } from "./ui";

const INITIALS_COLORS: AgentIconColor[] = ["red", "orange", "amber", "green", "teal", "blue", "indigo", "purple", "pink"];
const IMAGE_FILE = /\.(png|jpe?g|gif|webp|svg|ico)$/i;
const LIKELY_ICON = /(^|[/_.-])(logo|icon|favicon|brand|mark|avatar)/i;
const PROJECT_TABS = ["icons", "emoji", "image", "repo"] as const;

/** Two letters for a name: the first of its first two words, else its first two. */
export function projectInitials(name: string) {
  const words = name.replace(/(\p{Ll})(\p{Lu})/gu, "$1 $2").split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const letters = words.length > 1 ? words.slice(0, 2).map((word) => [...word][0]) : [...(words[0] ?? "?")].slice(0, 2);
  return letters.join("").toUpperCase();
}

/** Without an icon of its own, a project wears its initials on a colour its id picks, so neighbours differ. */
function defaultIcon(project: Project): ProjectIcon {
  let hash = 0;
  for (const char of project.id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return { brand: true, color: INITIALS_COLORS[hash % INITIALS_COLORS.length] };
}

const initialsMark = (name: string): Logo => (size) => <span className="font-bold leading-none" style={{ fontSize: Math.round(size * 0.8) }}>{projectInitials(name)}</span>;

export function ProjectIconTile({ project, icon = project.icon, size = "xs" }: {
  project: Project;
  /** An unsaved pick to show in place of the stored one. */
  icon?: ProjectIcon;
  size?: "sm" | "xs" | "md" | "lg";
}) {
  const { projectFileUrl } = useWorkspace().api;
  // A chat without a project has nothing to name; its tile says so on hover.
  if (project.scratch) return (
    <Tooltip content={m.no_project()} className={`inline-flex shrink-0 items-center justify-center text-muted ${TILE_SIZES[size].box}`}>
      <MessageCircleDashed size={TILE_SIZES[size].mark + 2} role="img" aria-label={m.no_project()} />
    </Tooltip>
  );
  return (
    <AgentIconTile
      icon={icon ?? defaultIcon(project)}
      brand={initialsMark(project.name)}
      size={size}
      fill
      fileUrl={(path) => projectFileUrl(project.id, path)}
    />
  );
}

/** The project's tile opening the icon picker; `name` is the one being typed, so the initials follow it. */
export function ProjectIconButton({ project, name, value, onChange, disabled }: {
  project: Project;
  name: string;
  value: ProjectIcon | undefined;
  onChange: (icon: ProjectIcon) => void;
  disabled?: boolean;
}) {
  const { projectFileUrl } = useWorkspace().api;
  const shown = { ...project, name };
  const icon = value ?? defaultIcon(project);
  return (
    <IconPickerButton tile={<ProjectIconTile project={shown} icon={icon} size="lg" />} disabled={disabled}>
      <IconPicker
        value={icon}
        brand={initialsMark(name)}
        brandLabel={m.icon_project_initials()}
        tabs={PROJECT_TABS}
        fileUrl={(path) => projectFileUrl(project.id, path)}
        repo={(pick) => <RepoImages projectId={project.id} selected={"file" in icon ? icon.file : null} onPick={pick} />}
        onChange={onChange}
      />
    </IconPickerButton>
  );
}

const CELL = "grid h-8.5 place-items-center rounded-lg border border-transparent outline-none hover:bg-hover-muted focus-visible:outline-2 focus-visible:outline-focus aria-checked:border-border aria-checked:bg-hover-muted";

/** The images in the project's checkout, logos and icons first. */
function RepoImages({ projectId, selected, onPick }: { projectId: string; selected: string | null; onPick: (file: string) => void }) {
  const { api: { projectFileUrl }, queries: { getCodeTreeQuery } } = useWorkspace();
  const tree = useQuery(getCodeTreeQuery(projectId));
  const [query, setQuery] = useState("");

  const search = query.trim().toLowerCase();
  const images = (tree.data?.entries ?? [])
    .filter((path) => IMAGE_FILE.test(path) && (!search || path.toLowerCase().includes(search)))
    .sort((left, right) => Number(LIKELY_ICON.test(right)) - Number(LIKELY_ICON.test(left)) || left.length - right.length);
  const shown = images.slice(0, 80);

  return (
    <div className="grid gap-2">
      <label className="relative block">
        <span className="sr-only">{m.icon_repo_search()}</span>
        <Search size={14} className="pointer-events-none absolute start-2.5 top-1/2 -translate-y-1/2 text-muted" aria-hidden="true" />
        <Input type="search" autoComplete="off" spellCheck={false} placeholder={m.icon_repo_search()} value={query} onChange={(event) => setQuery(event.target.value)} className="ps-8 [&::-webkit-search-cancel-button]:hidden" />
      </label>
      <div className="h-48 overflow-y-auto pe-1">
        {tree.isPending ? (
          <div role="status" className="grid h-full place-items-center"><Spinner /></div>
        ) : tree.isError ? (
          <div role="alert" className="grid justify-items-center gap-2 py-8 text-center text-sm text-subtext">
            {m.icon_repo_error()}
            <Button size="small" onClick={() => void tree.refetch()}>{m.app_retry()}</Button>
          </div>
        ) : shown.length === 0 ? (
          <p role="status" className="m-0 py-8 text-center text-sm text-subtext">{search ? m.icon_repo_no_match({ query: query.trim() }) : m.icon_repo_none()}</p>
        ) : (
          <div role="radiogroup" aria-label={m.icon_tab_repo()} className="grid grid-cols-[repeat(auto-fill,minmax(2rem,1fr))] gap-1">
            {shown.map((path) => (
              <button key={path} type="button" role="radio" aria-checked={path === selected} aria-label={path} title={path} className={CELL} onClick={() => onPick(path)}>
                <img alt="" draggable={false} loading="lazy" className="size-6 object-contain" src={projectFileUrl(projectId, path)} />
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
