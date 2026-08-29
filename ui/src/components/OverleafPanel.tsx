import { m } from "../paraglide/messages.js";
// Overleaf controls for a .tex tab, shown as a note strip under the file header
// like the compile log and the install hint beside it.
//
// The shape follows what Overleaf allows: syncing needs a Git authentication
// token and a project that already exists (its bridge cannot create one), and
// the bridge is a paid feature. So the upload route — which creates a new
// project and works on any account — is offered in every state, not only after
// a refusal.

import { useState } from "react";
import { ExternalLink } from "lucide-react";
import type { OverleafSync } from "../useOverleafSync";
import { getLocale } from "../paraglide/runtime.js";
import { autoDir, ltr } from "../i18n";
import { BUTTON_CLASS_NAME, GHOST_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

const list = (paths: string[]) => autoDir(new Intl.ListFormat(getLocale()).format(paths.map(ltr)));

/** What the last sync did. A failure outranks everything below it, so the line
 * never reads "in step" above a red one saying otherwise; conflicts carry their
 * own rows. */
function status(overleaf: OverleafSync): string {
  if (overleaf.error) return m.overleaf_last_sync_failed();
  if (overleaf.syncing) return m.overleaf_syncing();
  if (overleaf.blocked) return m.overleaf_save_to_sync();
  const last = overleaf.last;
  if (!last) return m.overleaf_paper_stays_in_sync();
  if (last.pulled.length && last.pushed.length) return m.overleaf_pulled_and_pushed({ pulled: list(last.pulled), pushed: list(last.pushed) });
  if (last.pulled.length) return m.overleaf_pulled({ paths: list(last.pulled) });
  if (last.pushed.length) return m.overleaf_pushed({ paths: list(last.pushed) });
  return last.conflicts.length ? m.overleaf_nothing_synced() : m.overleaf_in_sync();
}

function UploadLink({ href }: { href: string }) {
  return (
    <a className="text-sm text-subtext whitespace-nowrap" href={href} target="_blank" rel="noreferrer">
      {m.overleaf_panel_upload_a_copy_as_a_new_project()}
    </a>
  );
}

export function OverleafPanel({ overleaf }: { overleaf: OverleafSync }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [replacingToken, setReplacingToken] = useState(false);
  const replaceToken = () => {
    setValue("");
    setFormError(null);
    setReplacingToken(true);
  };
  const needsToken = !overleaf.hasToken || replacingToken;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const entered = value.trim();
    if (busy || !entered) return;
    setBusy(true);
    setFormError(null);
    try {
      if (needsToken) {
        await overleaf.saveToken(entered);
        setReplacingToken(false);
      } else {
        await overleaf.linkProject(entered);
      }
      setValue("");
    } catch (err) {
      setFormError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (overleaf.link && !replacingToken) {
    const conflicts = overleaf.last?.conflicts ?? [];
    return (
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center flex-wrap gap-2 text-sm text-subtext">
          <span className="flex-1 min-w-0">{status(overleaf)}</span>
          {overleaf.syncing && <span className={SPINNER_CLASS_NAME} />}
          <a
            className="inline-flex items-center gap-1 text-sm text-subtext whitespace-nowrap"
            href={overleaf.link.url}
            target="_blank"
            rel="noreferrer"
          >
            {m.overleaf_panel_open_in_overleaf()} <ExternalLink size={11} />
          </a>
          <button
            className={BUTTON_CLASS_NAME}
            disabled={overleaf.syncing || overleaf.blocked}
            data-tip={overleaf.blocked ? m.overleaf_save_first() : undefined}
            onClick={() => overleaf.sync()}
          >
            {m.overleaf_panel_sync_now()}
          </button>
          <button
            className={GHOST_BUTTON_CLASS_NAME}
            disabled={overleaf.syncing}
            onClick={() =>
              void overleaf.unlink().catch((err: unknown) => {
                setFormError(err instanceof Error ? err.message : String(err));
              })
            }
          >
            {m.overleaf_panel_unlink()}
          </button>
        </div>
        {conflicts.map((path) => (
          <div key={path} className="flex items-center flex-wrap gap-2 text-sm text-accent-red">
            <span className="flex-1 min-w-0">
              <code className="font-mono">{path}</code> {m.overleaf_panel_changed_here_and_on_overleaf_both_copies_are()}
            </span>
            <button
              className={BUTTON_CLASS_NAME}
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "keep-local" })}
            >
              {m.overleaf_panel_keep_this_copy()}
            </button>
            <button
              className={BUTTON_CLASS_NAME}
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "take-overleaf" })}
            >
              {m.overleaf_panel_use_overleaf_apos_s()}
            </button>
          </div>
        ))}
        {overleaf.last?.note && (
          <div className="text-sm text-accent-amber">{overleaf.last.note}</div>
        )}
        {formError && (
          <div className="text-sm text-accent-red whitespace-pre-wrap">{formError}</div>
        )}
        <div className="flex items-center flex-wrap gap-3">
          <UploadLink href={overleaf.uploadUrl} />
          <button type="button" className={GHOST_BUTTON_CLASS_NAME} onClick={replaceToken}>
            {m.overleaf_panel_replace_the_overleaf_token()}
          </button>
        </div>
      </div>
    );
  }

  return (
    <form className="flex flex-col gap-1.5" onSubmit={submit}>
      <div className="text-sm text-subtext">
        {needsToken
          ? m.overleaf_token_instructions()
          : m.overleaf_url_instructions()}
      </div>
      <div className="flex items-center flex-wrap gap-2">
        <input
          className="flex-1 min-w-55 font-mono text-sm"
          type={needsToken ? "password" : "text"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={needsToken ? m.overleaf_git_token() : "https://www.overleaf.com/project/…"}
          autoComplete="off"
        />
        <button type="submit" className={BUTTON_CLASS_NAME} disabled={busy || !value.trim()}>
          {busy ? (needsToken ? m.common_saving() : m.common_checking()) : needsToken ? m.overleaf_save_token() : m.overleaf_link_and_sync()}
        </button>
        <a
          className="text-sm text-subtext whitespace-nowrap"
          href={
            needsToken
              ? "https://www.overleaf.com/user/settings"
              : "https://www.overleaf.com/project"
          }
          target="_blank"
          rel="noreferrer"
        >
          {needsToken ? m.overleaf_create_token() : m.overleaf_my_projects()}
        </a>
      </div>
      {formError && <div className="text-sm text-accent-red whitespace-pre-wrap">{formError}</div>}
      <div className="flex items-center flex-wrap gap-3">
        <UploadLink href={overleaf.uploadUrl} />
        {replacingToken ? (
          <button
            type="button"
            className={GHOST_BUTTON_CLASS_NAME}
            onClick={() => setReplacingToken(false)}
          >
            {m.overleaf_panel_cancel()}
          </button>
        ) : (
          // A token can be rejected before this paper is ever linked, so the
          // way to replace it has to live in this state too.
          overleaf.hasToken && (
            <button type="button" className={GHOST_BUTTON_CLASS_NAME} onClick={replaceToken}>
              {m.overleaf_panel_replace_the_overleaf_token()}
            </button>
          )
        )}
      </div>
    </form>
  );
}
