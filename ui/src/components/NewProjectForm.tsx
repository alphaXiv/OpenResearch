import { useEffect, useRef, useState } from "react";
import { ChevronRight, FolderOpen } from "lucide-react";
import {
  createProject,
  getProjectPathStatus,
  pickProjectFolder,
  resolvePaper,
  searchPapers,
  type PaperHit,
  type Project,
  type ProjectPathStatus,
  type ResolvedPaper,
} from "../api";
import { BUTTON_CLASS_NAME, MONO_CLASS_NAME, PAPER_TITLE_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SMALL_BUTTON_CLASS_NAME } from "../styleClasses";

function slugify(text: string): string {
  return (
    text
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "research-project"
  );
}

function parsePaperId(input: string): string | null {
  const last = input.trim().split(/[?#]/)[0].split("/").filter(Boolean).pop() ?? "";
  const id = last.replace(/\.(pdf|md)$/i, "");
  return /^\d{4}\.\d{4,5}(v\d+)?$/.test(id) ? id : null;
}

type Mode = "folder" | "paper";
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
  const [mode, setMode] = useState<Mode>("folder");
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
  const [paperQuery, setPaperQuery] = useState("");
  const [paper, setPaper] = useState<ResolvedPaper | null>(null);
  const [hits, setHits] = useState<PaperHit[]>([]);
  const [searching, setSearching] = useState(false);
  const seq = useRef(0);
  const pathSeq = useRef(0);
  const folderPickSeq = useRef(0);
  const drafts = useRef<Record<Mode, ProjectDraft>>({
    folder: { name: "", nameTouched: false, path: "", pathTouched: false },
    paper: { name: "", nameTouched: false, path: "", pathTouched: false },
  });

  useEffect(() => {
    if (mode !== "paper" || !paper || pathTouched) return;
    const nextPath = `~/OpenResearch/${slugify(name || paper.title || paper.paperId)}`;
    if (nextPath === path) return;
    setPathStatus(null);
    setCheckingPath(true);
    setPath(nextPath);
  }, [mode, name, paper, path, pathTouched]);

  useEffect(() => {
    const request = ++pathSeq.current;
    setCheckingPath(true);
    setPathError(null);
    const timer = setTimeout(() => {
      void getProjectPathStatus(path.trim())
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
    }, path.trim() ? 200 : 0);
    return () => clearTimeout(timer);
  }, [mode, path]);

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
      setSearching(false);
      return;
    }
    setSearching(true);
    const timer = setTimeout(() => {
      if (id) {
        void resolvePaper(id)
          .then((resolved) => {
            if (request !== seq.current) return;
            setPaper(resolved);
            if (!nameTouched) setName(resolved.title?.trim().slice(0, 60) || resolved.paperId);
          })
          .catch((err) => request === seq.current && setError(err instanceof Error ? err.message : String(err)))
          .finally(() => request === seq.current && setSearching(false));
        return;
      }
      void searchPapers(query)
        .then((results) => request === seq.current && setHits(results))
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
      if (!nameTouched) setName(resolved.title?.trim().slice(0, 60) || resolved.paperId);
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
    setSearching(false);
    setPickingFolder(false);
    setPath("");
    setPathTouched(false);
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
      if (selected !== path) {
        setPathStatus(null);
        setCheckingPath(true);
        setPath(selected);
      }
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
        path: path.trim(),
        createFolder: mode === "paper",
        initializeGit: true,
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
    Boolean(path.trim()) &&
    pathStatus !== null &&
    pathStatus.exists === false;
  const invalidProjectDestination =
    Boolean(path.trim()) && pathStatus?.exists === true && pathStatus.directory === false;
  const nonemptyPaperCloneFolder =
    mode === "paper" && Boolean(paper?.repoUrl) && pathStatus?.empty === false;
  const paperDestinationHasError = invalidProjectDestination || nonemptyPaperCloneFolder;
  const paperDestinationDescription = invalidProjectDestination
    ? "Choose a different destination. This path is a file, not a folder."
    : nonemptyPaperCloneFolder
      ? "Choose a different destination. The paper repository needs a new or empty folder."
      : paper?.repoUrl
        ? pathStatus?.exists === false
          ? "OpenResearch will create this folder and clone the paper's repository into it."
          : "OpenResearch will clone the paper's repository here and use it as your local workspace."
        : pathStatus?.exists === false
          ? "OpenResearch will create this folder and initialize it as the local project workspace."
          : "OpenResearch will use this folder as the local workspace and initialize Git if needed.";
  const canCreate =
    Boolean(name.trim() && path.trim()) &&
    !pending &&
    !pickingFolder &&
    !checkingPath &&
    pathStatus !== null &&
    !pathError &&
    !gitMissing &&
    !missingLocalFolder &&
    !invalidProjectDestination &&
    !nonemptyPaperCloneFolder &&
    (mode !== "paper" || paper !== null);

  return (
    <form className="form [&_.form-seg]:[align-self:flex-start] [&_.form-seg]:[margin-bottom:2px] [&_.form-seg_button]:[padding:5px_12px] [&_.repo-hint]:[font-family:var(--mono)] [&_.repo-hint]:[font-weight:var(--fw-regular)] [&_.repo-hint]:[font-size:var(--fs-xs)] [&_.repo-hint]:[color:var(--muted)] [&_.repo-hint.ok]:[color:var(--accent-teal)] [&_.folder-picker-control]:[display:flex] [&_.folder-picker-control]:[align-items:center] [&_.folder-picker-control]:[gap:9px] [&_.folder-picker-control]:[width:100%] [&_.folder-picker-control]:[min-width:0] [&_.folder-picker-control]:[padding:8px_10px] [&_.folder-picker-control]:[overflow:hidden] [&_.folder-picker-control]:[background:var(--base)] [&_.folder-picker-control]:[border:1px_solid_var(--border)] [&_.folder-picker-control]:[border-radius:var(--radius-md)] [&_.folder-picker-control]:[cursor:pointer] [&_.folder-picker-control]:[text-align:left] [&_.folder-picker-control]:[transition:border-color_120ms_ease,_box-shadow_120ms_ease] [&_.folder-picker-control:hover:not(:disabled)]:[border-color:var(--muted)] [&_.folder-picker-control:hover:not(:disabled)]:[box-shadow:0_2px_8px_rgb(0_0_0_/_5%)] [&_.folder-picker-control:focus-visible]:[outline:2px_solid_var(--text)] [&_.folder-picker-control:focus-visible]:[outline-offset:2px] [&_.folder-picker-control_span]:[flex:1] [&_.folder-picker-control_span]:[min-width:0] [&_.folder-picker-control_span]:[overflow:hidden] [&_.folder-picker-control_span]:[text-overflow:ellipsis] [&_.folder-picker-control_span]:[white-space:nowrap] [&_.folder-picker-control_.placeholder]:[color:var(--muted)] [&_.folder-picker-icon]:[flex:none] [&_.folder-picker-icon]:[color:currentColor] [&_.folder-picker-chevron]:[flex:none] [&_.folder-picker-chevron]:[color:var(--muted)] [&_.folder-picker-control:hover:not(:disabled)_.folder-picker-chevron]:[color:var(--subtext)] [&_.folder-picker-hint]:[color:var(--subtext)] [&_.folder-picker-hint]:[font-size:var(--fs-sm)] [&_.folder-picker-hint]:[font-weight:var(--fw-regular)] [&_.folder-picker-hint]:[line-height:1.4] [&_.project-location-field]:[display:flex] [&_.project-location-field]:[flex-direction:column] [&_.project-location-field]:[gap:8px] [&_.project-location-label]:[color:var(--text)] [&_.project-location-label]:[font-size:var(--fs-base)] [&_.project-location-label]:[font-weight:var(--fw-semibold)] [&_.project-field-label]:[color:var(--text)] [&_.project-field-label]:[font-size:var(--fs-base)] [&_.project-field-label]:[font-weight:var(--fw-semibold)] [&_.folder-picker-control:disabled]:[cursor:default] [&_.folder-picker-control:disabled]:[opacity:0.65] [&_.paper-destination]:[display:flex] [&_.paper-destination]:[align-items:center] [&_.paper-destination]:[gap:10px] [&_.paper-destination]:[padding:8px_8px_8px_12px] [&_.paper-destination]:[border:1px_solid_var(--border)] [&_.paper-destination]:[border-radius:var(--radius-md)] [&_.paper-destination]:[background:var(--base)] [&_.paper-destination_code]:[flex:1] [&_.paper-destination_code]:[min-width:0] [&_.paper-destination_code]:[overflow:hidden] [&_.paper-destination_code]:[color:var(--text)] [&_.paper-destination_code]:[font-size:var(--fs-sm)] [&_.paper-destination_code]:[font-weight:var(--fw-regular)] [&_.paper-destination_code]:[text-overflow:ellipsis] [&_.paper-destination_code]:[white-space:nowrap] [&_.paper-destination_.btn]:[flex:none] [&_.project-path-notice]:[padding:9px_11px] [&_.project-path-notice]:[border:1px_solid_var(--border-variant)] [&_.project-path-notice]:[border-radius:var(--radius-sm)] [&_.project-path-notice]:[background:var(--surface)] [&_.project-path-notice]:[color:var(--subtext)] [&_.project-path-notice]:[font-size:var(--fs-sm)] [&_.project-path-notice]:[line-height:1.4] [&_.project-path-notice.error]:[border-color:color-mix(in_srgb,_var(--accent-red)_35%,_var(--border-variant))] [&_.paper-results]:[display:flex] [&_.paper-results]:[flex-direction:column] [&_.paper-results]:[border:1px_solid_var(--border)] [&_.paper-results]:[border-radius:var(--radius-md)] [&_.paper-results]:[max-height:240px] [&_.paper-results]:[overflow-y:auto] [&_.paper-results_button]:[display:flex] [&_.paper-results_button]:[flex-direction:column] [&_.paper-results_button]:[align-items:flex-start] [&_.paper-results_button]:[gap:2px] [&_.paper-results_button]:[padding:8px_10px] [&_.paper-results_button]:[background:none] [&_.paper-results_button]:[border:none] [&_.paper-results_button]:[border-bottom:1px_solid_var(--border-variant)] [&_.paper-results_button]:[text-align:left] [&_.paper-results_button]:[font:inherit] [&_.paper-results_button]:[color:var(--text)] [&_.paper-results_button]:[cursor:pointer] [&_.paper-results_button:last-child]:[border-bottom:none] [&_.paper-results_button:hover]:[background:var(--surface)] [&_.paper-results_.title]:[font-size:var(--fs-md)] [&_.paper-results_.title]:[font-weight:var(--fw-medium)] [&_.paper-results_.id]:[font-family:var(--mono)] [&_.paper-results_.id]:[font-size:var(--fs-xs)] [&_.paper-results_.id]:[color:var(--muted)] [&_.paper-pick_.id]:[font-family:var(--mono)] [&_.paper-pick_.id]:[font-size:var(--fs-xs)] [&_.paper-pick_.id]:[color:var(--muted)] [&_.paper-pick]:[display:flex] [&_.paper-pick]:[align-items:center] [&_.paper-pick]:[justify-content:space-between] [&_.paper-pick]:[gap:10px] [&_.paper-pick]:[padding:10px_12px] [&_.paper-pick]:[border:1px_solid_var(--border)] [&_.paper-pick]:[border-radius:var(--radius-md)] [&_.paper-pick]:[background:var(--surface)] [&_.paper-pick_.meta]:[min-width:0] [&_.paper-pick_.title]:[font-size:var(--fs-md)] [&_.paper-pick_.title]:[font-weight:var(--fw-semibold)] [display:flex] [flex-direction:column] [&_label]:[display:flex] [&_label]:[flex-direction:column] [&_label]:[gap:4px] [&_label]:[font-size:var(--fs-xs)] [&_label]:[color:var(--text)] [&_label]:[font-weight:var(--fw-medium)] [&_.row2]:[display:grid] [&_.row2]:[grid-template-columns:1fr_1fr] [&_.row2]:[gap:10px] [&_.actions]:[display:flex] [&_.actions]:[justify-content:flex-end] [&_.actions]:[gap:10px] [&_.actions]:[margin-top:6px] [&_.new-project-actions]:[justify-content:flex-start] [&_.new-project-actions]:[margin-top:10px] [&_.new-project-actions_.primary]:[margin-left:auto] [&_.error]:[color:var(--accent-red)] [&_.error]:[font-size:var(--fs-md)] [&_.error]:[white-space:pre-wrap] new-project-form [gap:18px] [&_>_label]:[gap:8px]" onSubmit={submit}>
      <div className="seg [display:inline-flex] [align-items:center] [gap:2px] [padding:3px] [border-radius:var(--radius-md)] [background:color-mix(in_oklab,_var(--text)_10%,_transparent)] [&_button]:[padding:3px_12px] [&_button]:[font-size:var(--fs-md)] [&_button]:[font-weight:var(--fw-semibold)] [&_button]:[color:var(--text)] [&_button]:[border-radius:var(--radius-sm)] [&_button:not(:disabled):hover]:[color:var(--text)] [&_button.active]:[background:var(--base)] [&_button.active]:[box-shadow:0_1px_3px_color-mix(in_oklab,_var(--text)_25%,_transparent)] [&_button:disabled]:[color:var(--muted)] [&_button:disabled]:[cursor:default] form-seg">
        <button
          type="button"
          className={mode === "folder" ? "active" : ""}
          aria-pressed={mode === "folder"}
          onClick={() => chooseMode("folder")}
        >
          From folder
        </button>
        <button
          type="button"
          className={mode === "paper" ? "active" : ""}
          aria-pressed={mode === "paper"}
          onClick={() => chooseMode("paper")}
        >
          From a paper
        </button>
      </div>

      {mode === "paper" && !paper && (
        <label>
          Paper
          <input
            value={paperQuery}
            onChange={(event) => setPaperQuery(event.target.value)}
            placeholder="arXiv id, URL, or title"
            autoFocus
          />
          <span className="repo-hint">{searching ? "Searching alphaXiv…" : "The public code repository is cloned without credentials."}</span>
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
        <div className="paper-pick">
          <div className="meta">
            <div className={PAPER_TITLE_CLASS_NAME}>{paper.title || paper.paperId}</div>
            <div className="id">
              {paper.repoUrl ? "Public code repository found" : "No public code repository found"}
            </div>
          </div>
          <button type="button" className={SMALL_BUTTON_CLASS_NAME} aria-label="Change selected paper" onClick={changePaper}>
            Change
          </button>
        </div>
      )}

      {(mode !== "paper" || paper) && (
        <>
          {mode === "paper" ? (
            <div className="project-location-field">
              <div className="project-location-label">
                {paper?.repoUrl ? "Clone destination" : "Project location"}
              </div>
              <div className="paper-destination">
                <code title={path}>{path}</code>
                <button
                  type="button"
                  className={SMALL_BUTTON_CLASS_NAME}
                  aria-label={`${paper?.repoUrl ? "Change clone destination" : "Change project location"}; current location: ${path}`}
                  aria-describedby="paper-destination-description"
                  disabled={pickingFolder}
                  onClick={() => void chooseLocalFolder()}
                >
                  {pickingFolder ? "Choosing…" : "Change…"}
                </button>
              </div>
              <span
                id="paper-destination-description"
                className={`folder-picker-hint${paperDestinationHasError ? " error" : ""}`}
              >
                {paperDestinationDescription}
              </span>
            </div>
          ) : (
            <button
              type="button"
              className="folder-picker-control"
              aria-label={path ? `Change project folder; current folder: ${path}` : "Choose or create a project folder"}
              disabled={pickingFolder}
              title={path || undefined}
              onClick={() => void chooseLocalFolder()}
            >
              <FolderOpen className={path ? "folder-picker-icon" : "folder-picker-icon placeholder"} size={16} />
              <span className={path ? MONO_CLASS_NAME : "placeholder"}>
                {pickingFolder ? "Choosing…" : path || "Choose or create a folder"}
              </span>
              <ChevronRight className="folder-picker-chevron" size={15} />
            </button>
          )}
          {path && (
            <label>
              <span className="project-field-label">Project name</span>
              <input
                value={name}
                onChange={(event) => {
                  setNameTouched(true);
                  setName(event.target.value);
                }}
                placeholder="my-research"
                autoFocus
              />
            </label>
          )}
          {gitMissing && (
            <div className="project-path-notice error">
              Git is required for experiments but is not installed. Install Git, then restart OpenResearch.
            </div>
          )}
          {!gitMissing && path.trim() && checkingPath && (
            <span className={`repo-hint ${MONO_CLASS_NAME}`}>Checking folder…</span>
          )}
          {!gitMissing && mode === "folder" && path.trim() && !checkingPath && pathStatus?.exists === false && (
            <div className="project-path-notice error">Choose an existing folder.</div>
          )}
          {!gitMissing && mode === "folder" && path.trim() && !checkingPath && invalidProjectDestination && (
            <div className="project-path-notice error">The selected path is not a folder.</div>
          )}
          {!gitMissing &&
            mode === "folder" &&
            !checkingPath &&
            pathStatus?.directory &&
            pathStatus.initialized === false && (
            <div className="project-path-notice">
              This folder is not a Git repository. OpenResearch will initialize Git here.
            </div>
            )}
          {pathError && <div className="project-path-notice error">{pathError}</div>}
        </>
      )}

      {error && <div className="error">{error}</div>}
      <div className="actions new-project-actions">
        {onCancel && <button type="button" className={BUTTON_CLASS_NAME} onClick={onCancel}>Cancel</button>}
        <button className={PRIMARY_BUTTON_CLASS_NAME} disabled={!canCreate}>
          {pending
            ? "Creating…"
            : mode === "paper"
              ? paper?.repoUrl
                ? "Clone paper project"
                : "Create paper project"
              : "Create local project"}
        </button>
      </div>
    </form>
  );
}
