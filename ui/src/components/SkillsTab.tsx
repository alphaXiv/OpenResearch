import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { Download, FileUp, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  deleteLatexTemplate,
  deleteUserSkill,
  fmtBytes,
  fmtNumber,
  importHarnessSkill,
  listHarnessSkills,
  listLatexTemplates,
  listUserSkills,
  timeAgo,
  uploadLatexTemplate,
  uploadUserSkill,
  type HarnessSkill,
  type LatexTemplate,
  type Project,
  type SkillScope,
  type UserSkill,
} from "../api";
import {
  BADGE_CLASS_NAME,
  ICON_BUTTON_CLASS_NAME,
  SMALL_BUTTON_CLASS_NAME,
  SPINNER_CLASS_NAME,
} from "../styleClasses";

const MAX_UPLOAD_BYTES = 20 * 1024 * 1024;

const CARD_CLASS_NAME =
  "bg-background border border-border rounded-lg py-4 px-4.5 mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5 [&_h3]:text-sm [&_h3]:font-semibold [&_h3]:text-text";
const CARD_SUB_CLASS_NAME = "mt-0 mx-0 mb-3 text-muted text-md leading-normal";
const SKILL_ROW_CLASS_NAME =
  "flex items-start gap-3 py-2.5 border-t border-t-border first:border-t-0";
const SKILL_NAME_CLASS_NAME = "font-mono text-sm font-medium text-text";
const SKILL_DESC_CLASS_NAME = "mt-0.5 mb-0 text-xs leading-relaxed text-muted";

/** Read a File into base64 (strips the `data:...;base64,` prefix). */
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== "string") {
        reject(new Error("could not read file"));
        return;
      }
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("could not read file"));
    reader.readAsDataURL(file);
  });
}

function isAcceptedName(name: string): boolean {
  const lower = name.toLowerCase();
  return lower.endsWith(".md") || lower.endsWith(".markdown") || lower.endsWith(".zip");
}

const SCOPE_BUTTON_CLASS_NAME =
  "flex-1 flex flex-col gap-0.5 py-2.5 px-3 border rounded-md text-start text-sm font-medium cursor-pointer transition-[border-color,background] duration-120 disabled:opacity-50 disabled:cursor-not-allowed";

function ScopePicker({
  scope,
  onScope,
  project,
  label,
}: {
  scope: SkillScope;
  onScope: (scope: SkillScope) => void;
  project: Project | null;
  label: string;
}) {
  const cls = (active: boolean) =>
    `${SCOPE_BUTTON_CLASS_NAME} ${
      active
        ? "border-primary bg-surface"
        : "border-border bg-background text-text [&:hover:not(:disabled)]:border-border-variant"
    }`;
  return (
    <div className="flex gap-2 mb-3.5" role="group" aria-label={label}>
      <button
        type="button"
        aria-pressed={scope === "global"}
        className={cls(scope === "global")}
        onClick={() => onScope("global")}
      >
        {m.skills_tab_global()}
        <span className="text-2xs font-normal text-muted">{m.skills_tab_every_project()}</span>
      </button>
      <button
        type="button"
        aria-pressed={scope === "project"}
        className={cls(scope === "project")}
        disabled={!project}
        title={project ? undefined : m.skills_open_project_to_scope()}
        onClick={() => onScope("project")}
      >
        {m.skills_tab_this_project()}
        <span className="text-2xs font-normal text-muted">
          {project ? project.name : m.skills_no_project_open()}
        </span>
      </button>
    </div>
  );
}

function DropZone({
  accept,
  busy,
  prompt,
  destination,
  onFile,
}: {
  accept: string;
  busy: boolean;
  prompt: ReactNode;
  destination: string;
  onFile: (file: File) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  return (
    <div
      className={`flex flex-col items-center justify-center gap-2 py-6.5 px-4.5 border-[1.5px] border-dashed rounded-md text-center text-sm transition-[border-color,background] duration-120 [&_code]:font-mono [&_code]:text-[0.92em] [&_code]:text-text ${
        busy ? "cursor-default" : "cursor-pointer"
      } ${
        dragging
          ? "border-primary bg-surface text-text"
          : "border-border-variant bg-surface text-muted [&:hover]:border-primary [&:hover]:text-text"
      }`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        const file = e.dataTransfer.files?.[0];
        if (file) onFile(file);
      }}
      onClick={() => inputRef.current?.click()}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          inputRef.current?.click();
        }
      }}
    >
      <input
        ref={inputRef}
        type="file"
        accept={accept}
        hidden
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) onFile(file);
          e.target.value = "";
        }}
      />
      {busy ? (
        <>
          <span className={SPINNER_CLASS_NAME} />
          <span>{m.skills_tab_uploading()}</span>
        </>
      ) : (
        <>
          <Upload size={20} strokeWidth={1.5} />
          <span>{prompt}</span>
          <span dir="auto" className="inline-flex items-center gap-1.5 text-2xs text-subtext">
            <FileUp size={12} /> {m.skills_tab_adding_to()}{" "}
            <strong dir="auto" className="text-text font-semibold">{destination}</strong>
          </span>
        </>
      )}
    </div>
  );
}

function SkillRow({
  skill,
  projectId,
  onDeleted,
  onError,
}: {
  skill: UserSkill;
  projectId?: string;
  onDeleted: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <div className={SKILL_ROW_CLASS_NAME}>
      <div className="flex-1 min-w-0">
        <code className={SKILL_NAME_CLASS_NAME}>/{skill.name}</code>
        <p className={SKILL_DESC_CLASS_NAME}>{skill.description}</p>
      </div>
      <div className="shrink-0 text-end whitespace-nowrap pt-0.5">
        <div className="text-2xs text-subtext">{fmtBytes(skill.bytes)}</div>
        {skill.updatedAt > 0 && (
          <div className="text-2xs text-muted">{timeAgo(skill.updatedAt)}</div>
        )}
      </div>
      <button
        className={ICON_BUTTON_CLASS_NAME}
        data-tip={m.skills_tab_delete_skill()}
        data-tip-align="end"
        aria-label={m.skills_delete_skill_label({ name: ltr(skill.name) })}
        disabled={busy}
        onClick={() => {
          if (!window.confirm(m.skills_delete_skill_confirm({ name: ltr(skill.name) }))) return;
          setBusy(true);
          deleteUserSkill({
            scope: skill.scope,
            name: skill.name,
            projectId: skill.scope === "project" ? projectId : undefined,
          })
            .then(onDeleted)
            .catch((e) => {
              setBusy(false);
              onError(e instanceof Error ? e.message : String(e));
            });
        }}
      >
        <Trash2 size={13} />
      </button>
    </div>
  );
}

function SkillList({
  title,
  hint,
  skills,
  projectId,
  onChanged,
  onError,
}: {
  title: string;
  hint: string;
  skills: UserSkill[];
  projectId?: string;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  return (
    <section className={CARD_CLASS_NAME}>
      <h3>{title}</h3>
      <p className={CARD_SUB_CLASS_NAME}>{hint}</p>
      {skills.length === 0 ? (
        <div className="text-muted text-sm">{m.skills_tab_no_skills_yet()}</div>
      ) : (
        <div className="flex flex-col">
          {skills.map((s) => (
            <SkillRow
              key={s.name}
              skill={s}
              projectId={projectId}
              onDeleted={onChanged}
              onError={onError}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function HarnessSkillRow({
  skill,
  scopeLabel,
  alreadyImported,
  onImport,
}: {
  skill: HarnessSkill;
  scopeLabel: string;
  alreadyImported: boolean;
  onImport: (skill: HarnessSkill) => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <div className={SKILL_ROW_CLASS_NAME}>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <code className={SKILL_NAME_CLASS_NAME}>/{skill.name}</code>
          <span className={BADGE_CLASS_NAME}>{skill.harnessName}</span>
        </div>
        <p className={SKILL_DESC_CLASS_NAME}>{skill.description}</p>
      </div>
      <button
        className={SMALL_BUTTON_CLASS_NAME}
        disabled={busy}
        title={m.a11y_import_into({ name: scopeLabel })}
        onClick={async () => {
          setBusy(true);
          try {
            await onImport(skill);
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? <span className={SPINNER_CLASS_NAME} /> : <Download size={13} />}
        {alreadyImported ? m.skills_reimport() : m.skills_import()}
      </button>
    </div>
  );
}

/** LaTeX templates the `orx-paper` skill follows instead of its built-in
 * preamble — a conference class, a lab style. Same store shape as skills:
 * Global, or scoped to the open project (which shadows a global). */
function LatexTemplatesCard({ project }: { project: Project | null }) {
  const [templates, setTemplates] = useState<LatexTemplate[] | null>(null);
  const [scope, setScope] = useState<SkillScope>("global");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    listLatexTemplates(project?.id)
      .then((next) => {
        setTemplates(next);
        setLoadError(null);
      })
      .catch((e) => {
        setTemplates([]);
        setLoadError(e instanceof Error ? e.message : String(e));
      });
  }, [project?.id]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (!project && scope === "project") setScope("global");
  }, [project, scope]);

  const busyRef = useRef(false);
  const upload = useCallback(
    async (file: File) => {
      if (busyRef.current) return;
      setError(null);
      const lower = file.name.toLowerCase();
      if (!lower.endsWith(".tex") && !lower.endsWith(".zip")) {
        setError(m.skills_upload_template_error());
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        setError(m.skills_file_too_large());
        return;
      }
      busyRef.current = true;
      setBusy(true);
      try {
        await uploadLatexTemplate({
          scope,
          projectId: scope === "project" ? project?.id : undefined,
          filename: file.name,
          contentBase64: await fileToBase64(file),
        });
        refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [scope, project?.id, refresh],
  );

  const applicable = templates ?? [];

  return (
    <section className={CARD_CLASS_NAME}>
      <h3>{m.skills_tab_la_te_x_templates()}</h3>
      <p className={`${CARD_SUB_CLASS_NAME} [&_code]:font-mono [&_code]:text-[0.92em] [&_code]:text-text`}>
        {m.skills_templates_description()}
      </p>

      <ScopePicker scope={scope} onScope={setScope} project={project} label={m.skills_template_scope()} />

      <DropZone
        accept=".tex,.zip"
        busy={busy}
        destination={scope === "global" ? m.skills_global_destination() : (project?.name ?? "")}
        prompt={m.skills_drop_template()}
        onFile={(file) => void upload(file)}
      />

      {error && <div className="mt-2.5 text-accent-red text-sm whitespace-pre-wrap">{error}</div>}

      {templates === null ? (
        <div className="flex items-center gap-2 text-subtext text-md pt-3">
          <span className={SPINNER_CLASS_NAME} /> {m.skills_tab_loading_templates()}
        </div>
      ) : loadError ? (
        <div className="text-accent-red text-sm pt-3">
          {m.skills_tab_could_not_load_templates()} {loadError}
        </div>
      ) : applicable.length === 0 ? (
        <div className="text-muted text-sm pt-3">{m.skills_tab_no_templates_yet()}</div>
      ) : (
        <div className="flex flex-col mt-1">
          {applicable.map((t) => (
            <LatexTemplateRow
              key={`${t.scope}:${t.name}`}
              template={t}
              projectId={project?.id}
              onChanged={refresh}
              onError={setError}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function LatexTemplateRow({
  template,
  projectId,
  onChanged,
  onError,
}: {
  template: LatexTemplate;
  projectId?: string;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const support = template.supportFiles.length;
  return (
    <div className={SKILL_ROW_CLASS_NAME}>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <code className={SKILL_NAME_CLASS_NAME}>{template.name}</code>
          <span className={BADGE_CLASS_NAME}>
            {template.scope === "global" ? m.skills_tab_global() : m.skills_tab_this_project()}
          </span>
        </div>
        <p className={SKILL_DESC_CLASS_NAME}>
          {template.entry}
          {support > 0 && (support === 1 ? m.skills_one_support_file() : m.skills_support_files({ count: fmtNumber(support) }))}
        </p>
      </div>
      <div className="shrink-0 text-end whitespace-nowrap pt-0.5">
        <div className="text-2xs text-subtext">{fmtBytes(template.bytes)}</div>
        {template.updatedAt > 0 && (
          <div className="text-2xs text-muted">{timeAgo(template.updatedAt)}</div>
        )}
      </div>
      <button
        className={ICON_BUTTON_CLASS_NAME}
        data-tip={m.skills_tab_delete_template()}
        data-tip-align="end"
        aria-label={m.skills_delete_template_label({ name: ltr(template.name) })}
        disabled={busy}
        onClick={() => {
          if (!window.confirm(m.skills_delete_template_confirm({ name: ltr(template.name) }))) return;
          setBusy(true);
          deleteLatexTemplate({
            scope: template.scope,
            name: template.name,
            projectId: template.scope === "project" ? projectId : undefined,
          })
            .then(onChanged)
            .catch((e) => {
              setBusy(false);
              onError(e instanceof Error ? e.message : String(e));
            });
        }}
      >
        <Trash2 size={13} />
      </button>
    </div>
  );
}

/** Middle-pane Customize tab — what the agent brings to every session: LaTeX
 * templates it writes papers into, SKILL.md skills it auto-discovers and the
 * user invokes with `/name`, and one-click import of skills already installed
 * in the user's coding agents. Each is Global (every project) or scoped to the
 * open one, which shadows a global of the same name. */
export function SkillsTab({ project }: { project: Project | null }) {
  const [skills, setSkills] = useState<UserSkill[] | null>(null);
  const [harnessSkills, setHarnessSkills] = useState<HarnessSkill[]>([]);
  const [scope, setScope] = useState<SkillScope>("global");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    listUserSkills(project?.id)
      .then(setSkills)
      .catch(() => setSkills([]));
  }, [project?.id]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Installed-agent skills don't change on user actions, so fetch once.
  useEffect(() => {
    listHarnessSkills()
      .then(setHarnessSkills)
      .catch(() => setHarnessSkills([]));
  }, []);

  // Project scope is unavailable without an open project.
  useEffect(() => {
    if (!project && scope === "project") setScope("global");
  }, [project, scope]);

  const busyRef = useRef(false);
  const upload = useCallback(
    async (file: File) => {
      if (busyRef.current) return; // ignore a second drop/pick mid-upload
      setError(null);
      if (!isAcceptedName(file.name)) {
        setError(m.skills_upload_skill_error());
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        setError(m.skills_file_too_large());
        return;
      }
      busyRef.current = true;
      setBusy(true);
      try {
        const contentBase64 = await fileToBase64(file);
        await uploadUserSkill({
          scope,
          projectId: scope === "project" ? project?.id : undefined,
          filename: file.name,
          contentBase64,
        });
        refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [scope, project?.id, refresh],
  );

  const importSkill = useCallback(
    async (skill: HarnessSkill) => {
      setError(null);
      try {
        await importHarnessSkill({
          harness: skill.harnessId,
          name: skill.name,
          scope,
          projectId: scope === "project" ? project?.id : undefined,
        });
        refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [scope, project?.id, refresh],
  );

  const globalSkills = (skills ?? []).filter((s) => s.scope === "global");
  const projectSkills = (skills ?? []).filter((s) => s.scope === "project");
  const scopeLabel = scope === "global" ? m.skills_tab_global() : (project?.name ?? m.skills_tab_this_project());
  // Names present in the scope an import would target — for the "Re-import" hint.
  const existingInScope = new Set(
    (skills ?? []).filter((s) => s.scope === scope).map((s) => s.name),
  );

  return (
    <div className="settings-view max-w-readable my-0 mx-auto pt-6 px-8 pb-15 [&_h1]:mt-0 [&_h1]:mx-0 [&_h1]:mb-1.5 [&_h1]:text-3xl">
      <h1>{m.skills_tab_customize()}</h1>
      <p className="mt-0 mx-0 mb-5 text-muted text-md leading-normal [&_code]:font-mono [&_code]:text-[0.92em] [&_code]:text-text">
        {m.skills_overview_description()}
      </p>

      <LatexTemplatesCard project={project} />

      <section className={CARD_CLASS_NAME}>
        <h3>{m.skills_tab_add_a_skill()}</h3>

        <ScopePicker scope={scope} onScope={setScope} project={project} label={m.skills_skill_scope()} />

        <DropZone
          accept=".md,.markdown,.zip"
          busy={busy}
          destination={scope === "global" ? m.skills_global_destination() : (project?.name ?? "")}
          prompt={m.skills_drop_skill()}
          onFile={(file) => void upload(file)}
        />

        {error && <div className="mt-2.5 text-accent-red text-sm whitespace-pre-wrap">{error}</div>}
      </section>

      {harnessSkills.length > 0 && (
        <section className={CARD_CLASS_NAME}>
          <h3>{m.skills_tab_import_from_your_agent()}</h3>
          <p dir="auto" className={`${CARD_SUB_CLASS_NAME} [&_code]:font-mono [&_code]:text-[0.92em] [&_code]:text-text [&_strong]:text-text [&_strong]:font-semibold`}>
            {m.skills_tab_skills_already_installed_in_your_coding_agents_import()}{" "}
            <strong>{scopeLabel}</strong> {m.skills_tab_so_it_s_managed_here_and_invocable_with()} <code>/name</code>.
          </p>
          <div className="flex flex-col">
            {harnessSkills.map((s) => (
              <HarnessSkillRow
                key={`${s.harnessId}:${s.name}`}
                skill={s}
                scopeLabel={scopeLabel}
                alreadyImported={existingInScope.has(s.name)}
                onImport={importSkill}
              />
            ))}
          </div>
        </section>
      )}

      {skills === null ? (
        <div className="flex items-center gap-2 text-subtext text-md p-3">
          <span className={SPINNER_CLASS_NAME} /> {m.skills_tab_loading_skills()}
        </div>
      ) : (
        <>
          <SkillList
            title={m.skills_tab_global_skills()}
            hint={m.skills_global_hint()}
            skills={globalSkills}
            onChanged={refresh}
            onError={setError}
          />
          {project && (
            <SkillList
              title={m.a11y_named_skills({ name: project.name })}
              hint={m.skills_project_hint()}
              skills={projectSkills}
              projectId={project.id}
              onChanged={refresh}
              onError={setError}
            />
          )}
        </>
      )}
    </div>
  );
}
