import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { RefreshCw, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  deleteLatexTemplate,
  deleteUserSkill,
  fmtBytes,
  fmtNumber,
  listLatexTemplates,
  listUserSkills,
  timeAgo,
  uploadLatexTemplate,
  uploadUserSkill,
  type LatexTemplate,
  type UserSkill,
} from "../api";
import { Badge, Button, IconButton, Spinner } from "./ui";

const MAX_UPLOAD_BYTES = 20 * 1024 * 1024;

const CARD_CLASS_NAME =
  "bg-background border border-border rounded-lg py-4 px-4.5 mb-4 [&_h3]:mt-0 [&_h3]:mx-0 [&_h3]:mb-2.5 [&_h3]:text-base [&_h3]:font-semibold [&_h3]:text-text";
const CARD_SUB_CLASS_NAME = "mt-0 mx-0 mb-3 text-sm leading-relaxed text-text";
const SKILL_ROW_CLASS_NAME =
  "flex items-start gap-3 py-2.5 border-t border-t-border first:border-t-0";
const SKILL_NAME_CLASS_NAME = "font-mono text-base font-medium text-text";
const ROW_DETAIL_CLASS_NAME = "mt-1 mb-0 text-sm leading-relaxed text-text";

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

function DropZone({
  accept,
  busy,
  prompt,
  onFile,
}: {
  accept: string;
  busy: boolean;
  prompt: ReactNode;
  onFile: (file: File) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  return (
    <div
      className={`flex flex-col items-center justify-center gap-2 py-6.5 px-4.5 border-[1.5px] border-dashed rounded-md text-center text-sm text-text transition-[border-color,background] duration-120 ${
        busy ? "cursor-default" : "cursor-pointer"
      } ${
        dragging
          ? "border-primary bg-surface text-text"
          : "border-border-variant bg-surface [&:hover]:border-primary"
      }`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        if (busy) return;
        const file = e.dataTransfer.files?.[0];
        if (file) onFile(file);
      }}
      onClick={() => {
        if (!busy) inputRef.current?.click();
      }}
      role="button"
      tabIndex={0}
      aria-disabled={busy}
      aria-busy={busy}
      onKeyDown={(e) => {
        if ((e.key === "Enter" || e.key === " ") && !busy) {
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
          <Spinner />
          <span>{m.skills_tab_uploading()}</span>
        </>
      ) : (
        <>
          <Upload size={20} strokeWidth={1.5} />
          <span>{prompt}</span>
        </>
      )}
    </div>
  );
}

/** Size and last-changed, shared by the skill and template rows. */
function RowMeta({ bytes, updatedAt }: { bytes: number; updatedAt: number }) {
  return (
    <div className="shrink-0 text-end whitespace-nowrap pt-0.5 text-xs text-subtext">
      {fmtBytes(bytes)}
      {updatedAt > 0 && <span className="text-muted"> · {timeAgo(updatedAt)}</span>}
    </div>
  );
}

/** One skill. A skill mirrored from a coding agent is managed where it lives,
 * so it carries that agent's badge instead of a delete button. */
function SkillRow({
  skill,
  onDeleted,
  onError,
}: {
  skill: UserSkill;
  onDeleted: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <div className={SKILL_ROW_CLASS_NAME}>
      <div className="flex-1 min-w-0 flex items-center gap-2">
        <code className={SKILL_NAME_CLASS_NAME}>/{skill.name}</code>
        {skill.origin && <Badge>{skill.origin}</Badge>}
      </div>
      <RowMeta bytes={skill.bytes} updatedAt={skill.updatedAt} />
      {!skill.origin && (
        <IconButton
          data-tip={m.skills_tab_delete_skill()}
          data-tip-align="end"
          aria-label={m.skills_delete_skill_label({ name: ltr(skill.name) })}
          disabled={busy}
          onClick={() => {
            if (!window.confirm(m.skills_delete_skill_confirm({ name: ltr(skill.name) }))) return;
            setBusy(true);
            deleteUserSkill(skill.name)
              .then(onDeleted)
              .catch((e) => {
                setBusy(false);
                onError(e instanceof Error ? e.message : String(e));
              });
          }}
        >
          <Trash2 size={13} />
        </IconButton>
      )}
    </div>
  );
}

function LatexTemplateRow({
  template,
  onChanged,
  onError,
}: {
  template: LatexTemplate;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const support = template.supportFiles.length;
  return (
    <div className={SKILL_ROW_CLASS_NAME}>
      <div className="flex-1 min-w-0">
        <span className="text-base font-medium text-text">{template.name}</span>
        <p className={ROW_DETAIL_CLASS_NAME}>
          {template.entry}
          {support > 0 &&
            (support === 1
              ? m.skills_one_support_file()
              : m.skills_support_files({ count: fmtNumber(support) }))}
        </p>
      </div>
      <RowMeta bytes={template.bytes} updatedAt={template.updatedAt} />
      <IconButton
        data-tip={m.skills_tab_delete_template()}
        data-tip-align="end"
        aria-label={m.skills_delete_template_label({ name: ltr(template.name) })}
        disabled={busy}
        onClick={() => {
          if (!window.confirm(m.skills_delete_template_confirm({ name: ltr(template.name) }))) return;
          setBusy(true);
          deleteLatexTemplate(template.name)
            .then(onChanged)
            .catch((e) => {
              setBusy(false);
              onError(e instanceof Error ? e.message : String(e));
            });
        }}
      >
        <Trash2 size={13} />
      </IconButton>
    </div>
  );
}

/** Everything the agent can invoke with `/name`: skills uploaded here, and the
 * ones already installed in the user's coding agents, mirrored automatically. */
function SkillsCard() {
  const [skills, setSkills] = useState<UserSkill[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setRefreshing(true);
    listUserSkills()
      .then((next) => {
        setSkills(next);
        setLoadError(null);
      })
      .catch((e) => {
        // An empty list is a real outcome here, so a failed fetch must not look
        // like one — it would read as "your agents' skills weren't found".
        setSkills([]);
        setLoadError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setRefreshing(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

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
        await uploadUserSkill({
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
    [refresh],
  );

  return (
    <section className={CARD_CLASS_NAME}>
      {/* Baseline-aligned so the heading's own bottom margin still spaces the card. */}
      <div className="flex items-baseline gap-2.5">
        <h3>{m.skills_tab_skills()}</h3>
        <Button className="ms-auto" size="small" onClick={refresh} disabled={refreshing}>
          <RefreshCw
            size={12}
            className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""}
         />{" "}
          {m.settings_page_refresh()}
        </Button>
      </div>
      <p className={CARD_SUB_CLASS_NAME}>{m.skills_description()}</p>

      <DropZone
        accept=".md,.markdown,.zip"
        busy={busy}
        prompt={m.skills_drop_skill()}
        onFile={(file) => void upload(file)}
     />

      {error && (
        <div role="alert" className="mt-2.5 text-base text-accent-red whitespace-pre-wrap">
          {error}
        </div>
      )}

      {skills === null ? (
        <div className="flex items-center gap-2 pt-3 text-sm text-subtext">
          <Spinner /> {m.skills_tab_loading_skills()}
        </div>
      ) : loadError ? (
        <div role="alert" className="pt-3 text-base text-accent-red">
          {m.skills_tab_could_not_load_skills()} {loadError}
        </div>
      ) : skills.length === 0 ? (
        <div className="pt-3 text-sm text-subtext">{m.skills_tab_no_skills_yet()}</div>
      ) : (
        <div className="flex flex-col mt-1">
          {skills.map((s) => (
            <SkillRow key={s.name} skill={s} onDeleted={refresh} onError={setError} />
          ))}
        </div>
      )}
    </section>
  );
}

/** LaTeX templates the `orx-paper` skill follows instead of its built-in
 * preamble — a conference class, a lab style. */
function LatexTemplatesCard() {
  const [templates, setTemplates] = useState<LatexTemplate[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    listLatexTemplates()
      .then((next) => {
        setTemplates(next);
        setLoadError(null);
      })
      .catch((e) => {
        setTemplates([]);
        setLoadError(e instanceof Error ? e.message : String(e));
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

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
    [refresh],
  );

  return (
    <section className={CARD_CLASS_NAME}>
      <h3>{m.skills_tab_la_te_x_templates()}</h3>
      <p className={CARD_SUB_CLASS_NAME}>{m.skills_templates_description()}</p>

      <DropZone
        accept=".tex,.zip"
        busy={busy}
        prompt={m.skills_drop_template()}
        onFile={(file) => void upload(file)}
     />

      {error && (
        <div role="alert" className="mt-2.5 text-base text-accent-red whitespace-pre-wrap">
          {error}
        </div>
      )}

      {templates === null ? (
        <div className="flex items-center gap-2 pt-3 text-sm text-subtext">
          <Spinner /> {m.skills_tab_loading_templates()}
        </div>
      ) : loadError ? (
        <div role="alert" className="pt-3 text-base text-accent-red">
          {m.skills_tab_could_not_load_templates()} {loadError}
        </div>
      ) : templates.length === 0 ? (
        <div className="pt-3 text-sm text-subtext">{m.skills_tab_no_templates_yet()}</div>
      ) : (
        <div className="flex flex-col mt-1">
          {templates.map((t) => (
            <LatexTemplateRow key={t.name} template={t} onChanged={refresh} onError={setError} />
          ))}
        </div>
      )}
    </section>
  );
}

/** Middle-pane Customize tab — what the agent brings to every session: the
 * skills it can invoke (uploaded here or mirrored from the user's coding
 * agents) and the LaTeX templates it writes papers into. Everything applies to
 * every project. */
export function SkillsTab() {
  return (
    <div className="settings-view max-w-readable my-0 mx-auto pt-6 px-8 pb-15 [&_h1]:mt-0 [&_h1]:mx-0 [&_h1]:mb-1.5 [&_h1]:text-3xl">
      <h1>{m.skills_tab_customize()}</h1>
      <p className="mt-0 mx-0 mb-5 text-base leading-relaxed text-text">
        {m.skills_overview_description()}
      </p>

      <SkillsCard />
      <LatexTemplatesCard />
    </div>
  );
}
