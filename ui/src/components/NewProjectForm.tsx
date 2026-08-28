import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, CircleAlert, FolderOpen } from "lucide-react";
import {
  createProject,
  githubAccount,
  githubProjectRepoPreview,
  getProjectDefaults,
  getProjectPathStatus,
  pickProjectFolder,
  repoAccess,
  resolvePaper,
  searchPapers,
  type PaperHit,
  type Project,
  type ProjectPathStatus,
  type ResolvedPaper,
} from "../api";
import { BUTTON_CLASS_NAME, MONO_CLASS_NAME, PAPER_TITLE_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SMALL_BUTTON_CLASS_NAME } from "../styleClasses";

function slugify(text: string, maxLength?: number): string {
  const slug = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return (maxLength ? slug.slice(0, maxLength) : slug) || "research-project";
}

function parsePaperId(input: string): string | null {
  const last = input.trim().split(/[?#]/)[0].split("/").filter(Boolean).pop() ?? "";
  const id = last.replace(/\.(pdf|md)$/i, "");
  return /^\d{4}\.\d{4,5}(v\d+)?$/.test(id) ? id : null;
}

function parseGithubRepository(url?: string | null): { owner: string; repo: string } | null {
  const match = url?.trim().match(/github\.com[/:]([^/]+)\/([^/?#]+)/i);
  if (!match) return null;
  return { owner: match[1], repo: match[2].replace(/\.git$/, "") };
}

function displayRepository(url: string): string {
  return url
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/^git@([^:]+):/i, "$1/")
    .replace(/\.git$/i, "")
    .replace(/\/$/, "");
}

type Mode = "blank" | "folder" | "paper";
type ProjectDraft = {
  name: string;
  nameTouched: boolean;
  path: string;
  pathTouched: boolean;
};

export function NewProjectForm({
  onCreated,
  onCancel,
}: {
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  onCancel?: () => void;
}) {
  const [mode, setMode] = useState<Mode>("blank");
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [path, setPath] = useState("");
  const [pathTouched, setPathTouched] = useState(false);
  const [pathStatus, setPathStatus] = useState<ProjectPathStatus | null>(null);
  const [pathError, setPathError] = useState<string | null>(null);
  const [checkingPath, setCheckingPath] = useState(false);
  const [pickingFolder, setPickingFolder] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [githubSyncEnabled, setGithubSyncEnabled] = useState(false);
  const [githubLogin, setGithubLogin] = useState<string | null | undefined>(undefined);
  const [githubRepoName, setGithubRepoName] = useState("research-project");
  const [writableGithubRepo, setWritableGithubRepo] = useState<string | null>(null);
  const [githubRepoPreviewPending, setGithubRepoPreviewPending] = useState(false);
  const [githubAccessPending, setGithubAccessPending] = useState(false);
  const [paperQuery, setPaperQuery] = useState("");
  const [paper, setPaper] = useState<ResolvedPaper | null>(null);
  const [hits, setHits] = useState<PaperHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [searchedPaperQuery, setSearchedPaperQuery] = useState("");
  const [pathCheckNonce, setPathCheckNonce] = useState(0);
  const seq = useRef(0);
  const pathSeq = useRef(0);
  const folderPickSeq = useRef(0);
  const drafts = useRef<Record<Mode, ProjectDraft>>({
    blank: { name: "", nameTouched: false, path: "", pathTouched: false },
    folder: { name: "", nameTouched: false, path: "", pathTouched: false },
    paper: { name: "", nameTouched: false, path: "", pathTouched: false },
  });
  const paperGithubRepo = mode === "paper" ? parseGithubRepository(paper?.repoUrl) : null;
  const automaticBlankProjectPath = name.trim() ? `~/OpenResearch/${slugify(name, 48)}` : "";
  const automaticPaperProjectPath = `~/OpenResearch/${slugify(name || paper?.title || paper?.paperId || "")}`;
  const projectPath = mode === "blank" && !pathTouched
    ? automaticBlankProjectPath
    : mode === "paper" && paper && !pathTouched
      ? automaticPaperProjectPath
      : path;
  const existingGithubRepo = paperGithubRepo ?? (
    pathStatus?.githubOwner && pathStatus.githubRepo
      ? { owner: pathStatus.githubOwner, repo: pathStatus.githubRepo }
      : null
  );

  useEffect(() => {
    void githubAccount()
      .then(({ login }) => setGithubLogin(login))
      .catch(() => setGithubLogin(null));
    void getProjectDefaults()
      .then((defaults) => setGithubSyncEnabled(defaults.githubForNewProjects))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    let current = true;
    setGithubRepoPreviewPending(true);
    const timer = setTimeout(() => {
      void githubProjectRepoPreview(name.trim())
        .then(({ repo }) => current && setGithubRepoName(repo))
        .catch(() => current && setGithubRepoName(slugify(name, 48)))
        .finally(() => current && setGithubRepoPreviewPending(false));
    }, 150);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [name]);

  useEffect(() => {
    let current = true;
    setWritableGithubRepo(null);
    setGithubAccessPending(Boolean(existingGithubRepo));
    if (!existingGithubRepo) return;
    void repoAccess(existingGithubRepo.owner, existingGithubRepo.repo)
      .then(({ canPush }) => {
        if (current && canPush) {
          setWritableGithubRepo(`github.com/${existingGithubRepo.owner}/${existingGithubRepo.repo}`);
        }
      })
      .catch(() => undefined)
      .finally(() => current && setGithubAccessPending(false));
    return () => {
      current = false;
    };
  }, [existingGithubRepo?.owner, existingGithubRepo?.repo]);

  useEffect(() => {
    const request = ++pathSeq.current;
    const trimmedPath = projectPath.trim();
    if (!trimmedPath) {
      setPathStatus(null);
      setPathError(null);
      setCheckingPath(false);
      return;
    }
    setCheckingPath(true);
    setPathError(null);
    const timer = setTimeout(() => {
      void getProjectPathStatus(trimmedPath)
        .then((status) => {
          if (request === pathSeq.current) setPathStatus(status);
        })
        .catch((err) => {
          if (request !== pathSeq.current) return;
          setPathStatus(null);
          setPathError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          if (request === pathSeq.current) setCheckingPath(false);
        });
    }, 200);
    return () => clearTimeout(timer);
  }, [mode, pathCheckNonce, projectPath]);

  useEffect(() => {
    const request = ++seq.current;
    if (mode !== "paper" || paper) {
      setSearching(false);
      return;
    }
    const query = paperQuery.trim();
    const id = parsePaperId(query);
    if (!id && query.length < 3) {
      setHits([]);
      setSearchedPaperQuery("");
      setSearching(false);
      return;
    }
    setError(null);
    setSearching(true);
    setHits([]);
    setSearchedPaperQuery("");
    const timer = setTimeout(() => {
      if (id) {
        void resolvePaper(id)
          .then((resolved) => {
            if (request !== seq.current) return;
            setPaper(resolved);
            if (!nameTouched) setName(resolved.title?.trim() || resolved.paperId);
          })
          .catch((err) => request === seq.current && setError(err instanceof Error ? err.message : String(err)))
          .finally(() => request === seq.current && setSearching(false));
        return;
      }
      void searchPapers(query)
        .then((results) => {
          if (request !== seq.current) return;
          setHits(results);
          setSearchedPaperQuery(query);
        })
        .catch((err) => request === seq.current && setError(err instanceof Error ? err.message : String(err)))
        .finally(() => request === seq.current && setSearching(false));
    }, 350);
    return () => clearTimeout(timer);
  }, [mode, paper, paperQuery, nameTouched]);

  async function choosePaper(paperId: string) {
    const request = ++seq.current;
    setSearching(true);
    setError(null);
    try {
      const resolved = await resolvePaper(paperId);
      if (request !== seq.current) return;
      setPaper(resolved);
      setHits([]);
      if (!nameTouched) setName(resolved.title?.trim() || resolved.paperId);
    } catch (err) {
      if (request === seq.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (request === seq.current) setSearching(false);
    }
  }

  function changePaper() {
    seq.current += 1;
    folderPickSeq.current += 1;
    setPaper(null);
    setPaperQuery("");
    setHits([]);
    setSearchedPaperQuery("");
    setSearching(false);
    setPickingFolder(false);
    setPath("");
    setPathTouched(false);
    drafts.current.paper = {
      name: nameTouched ? name : "",
      nameTouched,
      path: "",
      pathTouched: false,
    };
    if (!nameTouched) setName("");
  }

  function chooseMode(next: Mode) {
    if (next === mode) return;
    seq.current += 1;
    folderPickSeq.current += 1;
    drafts.current[mode] = { name, nameTouched, path, pathTouched };
    const nextDraft = drafts.current[next];
    setMode(next);
    setError(null);
    setPathError(null);
    setPathStatus(null);
    setSearching(false);
    setPickingFolder(false);
    setName(nextDraft.name);
    setNameTouched(nextDraft.nameTouched);
    setPath(nextDraft.path);
    setPathTouched(nextDraft.pathTouched);
  }

  async function chooseLocalFolder() {
    if (pickingFolder) return;
    const request = ++folderPickSeq.current;
    setPickingFolder(true);
    setError(null);
    try {
      const selected = await pickProjectFolder();
      if (request !== folderPickSeq.current || !selected) return;
      setPathTouched(true);
      setPathStatus(null);
      setCheckingPath(true);
      setPath(selected);
      setPathCheckNonce((nonce) => nonce + 1);
      if (mode === "folder" && !nameTouched) {
        const folderName = selected.replace(/[\\/]+$/, "").split(/[\\/]/).pop();
        if (folderName) setName(folderName);
      }
    } catch (err) {
      if (request === folderPickSeq.current) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (request === folderPickSeq.current) setPickingFolder(false);
    }
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!canCreate) return;
    setPending(true);
    setError(null);
    try {
      const result = await createProject({
        name: name.trim(),
        path: projectPath.trim(),
        createFolder: mode !== "folder",
        requireNewFolder: mode === "blank",
        initializeGit: true,
        githubSyncEnabled,
        ...(mode === "paper" && paper
          ? { paperId: paper.paperId, cloneUrl: paper.repoUrl ?? undefined }
          : {}),
      });
      onCreated(result.project, result.githubPublicationError);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPending(false);
    }
  }

  const gitMissing = pathStatus?.gitVersion === null;
  const missingLocalFolder =
    mode === "folder" &&
    Boolean(projectPath.trim()) &&
    pathStatus !== null &&
    pathStatus.exists === false;
  const existingBlankFolder = mode === "blank" && pathStatus?.exists === true;
  const invalidProjectDestination =
    Boolean(projectPath.trim()) && pathStatus?.exists === true && pathStatus.directory === false;
  const nonemptyPaperCloneFolder =
    mode === "paper" && Boolean(paper?.repoUrl) && pathStatus?.empty === false;
  // A blank paper project is seeded and committed at the folder it initializes,
  // so it needs a folder of its own rather than one inside an existing repo.
  const unusableBlankPaperFolder =
    mode === "paper" &&
    Boolean(paper) &&
    !paper?.repoUrl &&
    (pathStatus?.empty === false ||
      (pathStatus?.gitState != null && pathStatus.gitState !== "notRepository"));
  const unusableRepository =
    mode === "folder" &&
    (pathStatus?.gitState === "detached" || pathStatus?.gitState === "invalid");
  const paperDestinationHasError =
    (pathTouched && !projectPath.trim()) ||
    invalidProjectDestination ||
    nonemptyPaperCloneFolder ||
    unusableBlankPaperFolder;
  const blankDestinationHasError =
    (pathTouched && !projectPath.trim()) || invalidProjectDestination || existingBlankFolder;
  const blankDestinationError = pathTouched && !projectPath.trim()
    ? m.new_project_location_required()
    : invalidProjectDestination
    ? m.new_project_destination_is_file()
    : existingBlankFolder
      ? m.new_project_folder_exists()
      : null;
  const paperDestinationError = pathTouched && !projectPath.trim()
    ? m.new_project_location_required()
    : invalidProjectDestination
    ? m.new_project_destination_is_file()
    : nonemptyPaperCloneFolder
      ? m.new_project_paper_needs_empty_folder()
      : unusableBlankPaperFolder
        ? m.new_project_blank_paper_needs_folder()
        : null;
  const canCreate =
    Boolean(name.trim() && projectPath.trim()) &&
    !pending &&
    !pickingFolder &&
    !checkingPath &&
    pathStatus !== null &&
    !pathError &&
    !gitMissing &&
    !missingLocalFolder &&
    !existingBlankFolder &&
    !invalidProjectDestination &&
    !nonemptyPaperCloneFolder &&
    !unusableBlankPaperFolder &&
    !unusableRepository &&
    (mode !== "paper" || Boolean(paper)) &&
    (!githubSyncEnabled ||
      (typeof githubLogin === "string" &&
        !githubRepoPreviewPending &&
        !githubAccessPending));
  const githubRepository =
    writableGithubRepo ?? `github.com/${githubLogin ?? "you"}/${githubRepoName}`;
  const githubDecisionPending =
    githubLogin === undefined || githubRepoPreviewPending || githubAccessPending;
  const hasNoPaperResults =
    mode === "paper" &&
    !paper &&
    paperQuery.trim().length >= 3 &&
    searchedPaperQuery === paperQuery.trim() &&
    !searching &&
    hits.length === 0 &&
    !error;

  return (
    <form className="form [&_.form-seg]:self-start [&_.form-seg]:mb-0.5 [&_.form-seg_button]:py-[5px] [&_.form-seg_button]:px-3 [&_.repo-hint]:font-normal [&_.repo-hint]:text-md [&_.repo-hint]:text-muted [&_.repo-hint.ok]:text-accent-teal [&_.folder-picker-control]:flex [&_.folder-picker-control]:items-center [&_.folder-picker-control]:gap-[9px] [&_.folder-picker-control]:w-full [&_.folder-picker-control]:min-w-0 [&_.folder-picker-control]:py-2 [&_.folder-picker-control]:px-2.5 [&_.folder-picker-control]:overflow-hidden [&_.folder-picker-control]:bg-background [&_.folder-picker-control]:border [&_.folder-picker-control]:border-border [&_.folder-picker-control]:rounded-md [&_.folder-picker-control]:cursor-pointer [&_.folder-picker-control]:text-start [&_.folder-picker-control]:transition-[border-color,box-shadow] [&_.folder-picker-control]:duration-120 [&_.folder-picker-control]:ease-standard [&_.folder-picker-control:hover:not(:disabled)]:border-muted [&_.folder-picker-control:hover:not(:disabled)]:shadow-[0_2px_8px_rgb(0_0_0_/_5%)] [&_.folder-picker-control:focus-visible]:outline-2 [&_.folder-picker-control:focus-visible]:outline-solid [&_.folder-picker-control:focus-visible]:outline-text [&_.folder-picker-control:focus-visible]:outline-offset-2 [&_.folder-picker-control_span]:flex-1 [&_.folder-picker-control_span]:min-w-0 [&_.folder-picker-control_span]:overflow-hidden [&_.folder-picker-control_span]:text-ellipsis [&_.folder-picker-control_span]:whitespace-nowrap [&_.folder-picker-control_.placeholder]:text-muted [&_.folder-picker-icon]:flex-none [&_.folder-picker-icon]:text-current [&_.folder-picker-chevron]:flex-none [&_.folder-picker-chevron]:text-muted [&_.folder-picker-control:hover:not(:disabled)_.folder-picker-chevron]:text-subtext [&_.folder-picker-hint]:text-subtext [&_.folder-picker-hint]:text-sm [&_.folder-picker-hint]:font-normal [&_.folder-picker-hint]:leading-[1.4] [&_.project-location-field]:flex [&_.project-location-field]:flex-col [&_.project-location-field]:gap-2 [&_.project-location-label]:text-text [&_.project-location-label]:text-base [&_.project-location-label]:font-semibold [&_.project-field-label]:text-text [&_.project-field-label]:text-base [&_.project-field-label]:font-semibold [&_.folder-picker-control:disabled]:cursor-default [&_.folder-picker-control:disabled]:opacity-65 [&_.paper-destination]:flex [&_.paper-destination]:items-center [&_.paper-destination]:gap-2.5 [&_.paper-destination]:pt-2 [&_.paper-destination]:pe-2 [&_.paper-destination]:pb-2 [&_.paper-destination]:ps-3 [&_.paper-destination]:border [&_.paper-destination]:border-border [&_.paper-destination]:rounded-md [&_.paper-destination]:bg-background [&_.paper-destination_code]:flex-1 [&_.paper-destination_code]:min-w-0 [&_.paper-destination_code]:overflow-hidden [&_.paper-destination_code]:text-text [&_.paper-destination_code]:text-sm [&_.paper-destination_code]:font-normal [&_.paper-destination_code]:text-ellipsis [&_.paper-destination_code]:whitespace-nowrap [&_.paper-destination_.btn]:flex-none [&_.project-path-notice]:py-[9px] [&_.project-path-notice]:px-[11px] [&_.project-path-notice]:border [&_.project-path-notice]:border-border-variant [&_.project-path-notice]:rounded-sm [&_.project-path-notice]:bg-surface [&_.project-path-notice]:text-subtext [&_.project-path-notice]:text-sm [&_.project-path-notice]:leading-[1.4] [&_.project-path-notice.error]:border-[color-mix(in_srgb,_var(--accent-red)_35%,_var(--border-variant))] [&_.paper-results]:flex [&_.paper-results]:flex-col [&_.paper-results]:border [&_.paper-results]:border-border [&_.paper-results]:rounded-md [&_.paper-results]:max-h-60 [&_.paper-results]:overflow-y-auto [&_.paper-results_button]:flex [&_.paper-results_button]:flex-col [&_.paper-results_button]:items-start [&_.paper-results_button]:gap-0.5 [&_.paper-results_button]:py-2 [&_.paper-results_button]:px-2.5 [&_.paper-results_button]:bg-none [&_.paper-results_button]:bg-transparent [&_.paper-results_button]:border-0 [&_.paper-results_button]:border-b [&_.paper-results_button]:border-b-border-variant [&_.paper-results_button]:text-start [&_.paper-results_button]:[font:inherit] [&_.paper-results_button]:text-text [&_.paper-results_button]:cursor-pointer [&_.paper-results_button:last-child]:border-b-0 [&_.paper-results_button:hover]:bg-surface [&_.paper-results_.title]:text-md [&_.paper-results_.title]:font-medium [&_.paper-results_.id]:font-mono [&_.paper-results_.id]:text-xs [&_.paper-results_.id]:text-muted [&_.paper-pick_.id]:font-mono [&_.paper-pick_.id]:text-xs [&_.paper-pick_.id]:text-muted [&_.paper-pick]:flex [&_.paper-pick]:items-center [&_.paper-pick]:justify-between [&_.paper-pick]:gap-2.5 [&_.paper-pick]:py-2.5 [&_.paper-pick]:px-3 [&_.paper-pick]:border [&_.paper-pick]:border-border [&_.paper-pick]:rounded-md [&_.paper-pick]:bg-surface [&_.paper-pick_.meta]:min-w-0 [&_.paper-pick_.title]:text-md [&_.paper-pick_.title]:font-semibold flex flex-col [&_label]:flex [&_label]:flex-col [&_label]:gap-1 [&_label]:text-xs [&_label]:text-text [&_label]:font-medium [&_.row2]:grid [&_.row2]:grid-cols-2 [&_.row2]:gap-2.5 [&_.actions]:flex [&_.actions]:justify-end [&_.actions]:gap-2.5 [&_.actions]:mt-1.5 [&_.new-project-actions]:justify-start [&_.new-project-actions]:mt-2.5 [&_.new-project-actions_.primary]:ms-auto [&_.error]:text-accent-red [&_.error]:text-md [&_.error]:whitespace-pre-wrap new-project-form gap-4.5 [&_>_label]:gap-2" onSubmit={submit}>
      <div className="seg inline-flex items-center gap-0.5 p-[3px] rounded-md bg-[color-mix(in_oklab,_var(--text)_10%,_transparent)] [&_button]:py-[3px] [&_button]:px-3 [&_button]:text-md [&_button]:font-medium [&_button]:text-text [&_button]:rounded-sm [&_button:not(:disabled):hover]:text-text [&_button.active]:bg-background [&_button.active]:shadow-[0_1px_3px_color-mix(in_oklab,_var(--text)_25%,_transparent)] [&_button:disabled]:text-muted [&_button:disabled]:cursor-default form-seg">
        <button
          type="button"
          className={mode === "blank" ? "active" : ""}
          aria-pressed={mode === "blank"}
          onClick={() => chooseMode("blank")}
        >
          {m.new_project_form_blank_project()}
        </button>
        <span aria-hidden className={`h-6 w-px bg-border${mode === "paper" ? "" : " invisible"}`} />
        <button
          type="button"
          className={mode === "folder" ? "active" : ""}
          aria-pressed={mode === "folder"}
          onClick={() => chooseMode("folder")}
        >
          {m.new_project_form_existing_folder()}
        </button>
        <span aria-hidden className={`h-6 w-px bg-border${mode === "blank" ? "" : " invisible"}`} />
        <button
          type="button"
          className={mode === "paper" ? "active" : ""}
          aria-pressed={mode === "paper"}
          onClick={() => chooseMode("paper")}
        >
          {m.new_project_form_from_a_paper()}
        </button>
      </div>

      {mode === "paper" && !paper && (
        <label className="!font-normal">
          {m.new_project_form_paper()}
          <input
            className="text-md font-normal"
            data-initial-focus
            value={paperQuery}
            onChange={(event) => {
              setError(null);
              setSearchedPaperQuery("");
              setPaperQuery(event.target.value);
            }}
            placeholder={m.new_project_form_search_for_a_paper_by_ar_xiv_id()}
          />
          {!hasNoPaperResults && (
            <span className="repo-hint">{searching ? m.new_project_searching_alphaxiv() : m.new_project_public_repo_cloned()}</span>
          )}
          {hasNoPaperResults && (
            <span className="project-path-notice block">{m.new_project_form_no_papers_found_try_an_ar_xiv_id()}</span>
          )}
          {hits.length > 0 && (
            <div className="paper-results">
              {hits.map((hit) => (
                <button key={hit.paperId} type="button" onClick={() => void choosePaper(hit.paperId)}>
                  <span className={PAPER_TITLE_CLASS_NAME}>{hit.title}</span>
                  <span className="id">{hit.paperId}</span>
                </button>
              ))}
            </div>
          )}
        </label>
      )}

      {paper && mode === "paper" && (
        <div className="paper-pick !flex-col !items-stretch">
          <div className="flex items-start justify-between gap-2.5">
            <div className="meta">
              <div className={`${PAPER_TITLE_CLASS_NAME} !font-medium`}>{paper.title || paper.paperId}</div>
              {paper.repoUrl && <div className="id">{displayRepository(paper.repoUrl)}</div>}
            </div>
            <button type="button" className={SMALL_BUTTON_CLASS_NAME} aria-label={m.new_project_form_change_selected_paper()} onClick={changePaper}>
              {m.new_project_form_change()}
            </button>
          </div>
          {!paper.repoUrl && (
            <div className="flex w-full flex-col items-start gap-1 rounded-md border border-border-variant bg-background px-[9px] py-1 text-sm font-normal text-subtext">
              <span className="flex items-center gap-[5px] text-md">
                <CircleAlert size={16} /> {m.new_project_form_no_public_repository_found_on_alpha_xiv()}
              </span>
              <span className="text-sm font-normal text-accent-amber">{m.new_project_form_open_research_will_start_a_blank_project_with()}</span>
            </div>
          )}
        </div>
      )}

      {(mode !== "paper" || paper) && (
        <>
          {mode === "blank" && (
            <label className="!font-normal">
              <span className="project-field-label !font-medium">{m.new_project_form_project_name()}</span>
              <input
                className="text-md font-normal"
                data-initial-focus
                value={name}
                onChange={(event) => {
                  setNameTouched(true);
                  setName(event.target.value);
                }}
                placeholder={m.new_project_form_my_research()}
              />
            </label>
          )}
          {mode === "paper" ? (
            <label className="project-location-field">
              <span className="project-location-label !font-medium">
                {paper?.repoUrl ? m.new_project_clone_destination() : m.new_project_form_project_location()}
              </span>
              <input
                className="text-md font-normal"
                value={projectPath}
                onChange={(event) => {
                  setPathTouched(true);
                  setPathStatus(null);
                  setPath(event.target.value);
                }}
                aria-describedby={paperDestinationHasError ? "paper-destination-description" : undefined}
                placeholder="~/OpenResearch/paper-title"
                spellCheck={false}
              />
              {checkingPath && (
                <span className="sr-only" role="status" aria-live="polite">{m.new_project_form_checking_project_location()}</span>
              )}
              {paperDestinationHasError && (
                <span id="paper-destination-description" className="folder-picker-hint error !text-accent-red" role="alert">
                  {paperDestinationError}
                </span>
              )}
            </label>
          ) : mode === "folder" ? (
            <button
              data-initial-focus
              type="button"
              className="folder-picker-control"
              aria-label={path ? m.new_project_change_folder({ path: ltr(path) }) : m.new_project_choose_existing_folder()}
              disabled={pickingFolder}
              title={path || undefined}
              onClick={() => void chooseLocalFolder()}
            >
              <FolderOpen className={path ? "folder-picker-icon" : "folder-picker-icon placeholder"} size={16} />
              <span className={path ? MONO_CLASS_NAME : "placeholder"}>
                {pickingFolder ? m.new_project_choosing() : path || m.new_project_choose_existing_folder()}
              </span>
              <ChevronRight className="rtl-mirror folder-picker-chevron" size={15} />
            </button>
          ) : name.trim() ? (
            <label className="project-location-field">
              <span className="project-location-label !font-medium">{m.new_project_form_project_location()}</span>
              <input
                className="text-md font-normal"
                value={projectPath}
                onChange={(event) => {
                  setPathTouched(true);
                  setPathStatus(null);
                  setPath(event.target.value);
                }}
                placeholder="~/OpenResearch/my-research"
                aria-describedby={blankDestinationHasError ? "blank-destination-description" : undefined}
                spellCheck={false}
              />
              {checkingPath && (
                <span className="sr-only" role="status" aria-live="polite">{m.new_project_form_checking_project_location()}</span>
              )}
              {blankDestinationHasError && (
                <span id="blank-destination-description" className="folder-picker-hint error !text-accent-red" role="alert">
                  {blankDestinationError}
                </span>
              )}
            </label>
          ) : null}
          {mode !== "blank" && projectPath && (
            <label className="!font-normal">
              <span className="project-field-label !font-medium">{m.new_project_form_project_name()}</span>
              <input
                className="text-md font-normal"
                value={name}
                onChange={(event) => {
                  setNameTouched(true);
                  setName(event.target.value);
                }}
                placeholder={m.new_project_form_my_research()}
              />
            </label>
          )}
          {gitMissing && (
            <div className="project-path-notice error">
              {m.new_project_form_git_is_required_for_experiments_but_is_not()}
            </div>
          )}
          {!gitMissing && mode === "folder" && path.trim() && !checkingPath && pathStatus?.exists === false && (
            <div className="project-path-notice error">{m.new_project_form_that_folder_no_longer_exists_choose_it_again()}</div>
          )}
          {!gitMissing && mode === "folder" && path.trim() && !checkingPath && invalidProjectDestination && (
            <div className="project-path-notice error">{m.new_project_form_the_selected_path_is_not_a_folder()}</div>
          )}
          {!gitMissing && mode === "folder" && !checkingPath && pathStatus?.gitState === "detached" && (
            <div className="project-path-notice error">{m.new_project_form_check_out_a_git_branch_before_using_this()}</div>
          )}
          {!gitMissing && mode === "folder" && !checkingPath && pathStatus?.gitState === "invalid" && (
            <div className="project-path-notice error">{m.new_project_form_the_selected_folder_contains_an_invalid_git_repository()}</div>
          )}
          {pathError && <div className="project-path-notice error" role="alert">{pathError}</div>}
        </>
      )}

      {error && <div className="error" role="alert">{error}</div>}
      {(mode !== "paper" || paper) && projectPath && (mode !== "blank" || name.trim()) && (
        <div className="flex w-full flex-col items-start gap-2">
          <button
            type="button"
            className={`inline-flex items-center gap-1 text-md font-medium${githubSyncEnabled && githubLogin === null ? " text-accent-red" : " text-text"}`}
            aria-expanded={advancedOpen}
            aria-controls="new-project-advanced-settings"
            onClick={() => setAdvancedOpen((open) => !open)}
          >
            {githubSyncEnabled
              ? githubLogin === null
                ? m.new_project_advanced_connect_github()
                : m.new_project_advanced_sync_on()
              : m.new_project_advanced()}
            <ChevronDown className={advancedOpen ? "rotate-180" : ""} size={16} />
          </button>
          {advancedOpen && (
            <label id="new-project-advanced-settings" className="flex w-full flex-col items-stretch gap-[7px] font-normal">
              <span className="flex flex-row items-center gap-[9px]">
                <input
                  className="m-0"
                  type="checkbox"
                  checked={githubSyncEnabled}
                  onChange={(event) => setGithubSyncEnabled(event.target.checked)}
                  disabled={pending}
                />
                <strong className="text-base font-medium leading-[1.3] text-text">
                  {m.new_project_form_sync_experiments_to_git_hub()}
                </strong>
              </span>
              <span className="flex flex-col gap-[3px] font-sans text-sm font-normal leading-[1.4] text-subtext">
                <span>
                  {githubDecisionPending
                    ? m.new_project_github_checking({ repository: ltr(githubRepository) })
                    : writableGithubRepo
                      ? m.new_project_github_pushes({ repository: ltr(githubRepository) })
                      : m.new_project_github_creates({ repository: ltr(githubRepository) })}
                </span>
                <span>{m.new_project_form_experiment_branches_will_be_pushed_to_the_remote()}</span>
                {githubLogin === null && <span>{m.new_project_run_before_create({ command: ltr("gh auth login") })}</span>}
              </span>
            </label>
          )}
        </div>
      )}
      <div className="actions new-project-actions">
        {onCancel && <button type="button" className={`${BUTTON_CLASS_NAME} !font-medium`} onClick={onCancel}>{m.new_project_form_cancel()}</button>}
        <button className={`${PRIMARY_BUTTON_CLASS_NAME} !font-medium`} disabled={!canCreate}>
          {pending
            ? m.new_project_creating()
            : mode === "paper"
              ? paper?.repoUrl
                ? m.new_project_clone_paper()
                : m.new_project_create()
              : mode === "folder"
                ? m.new_project_use_folder()
                : m.new_project_create()}
        </button>
      </div>
    </form>
  );
}
