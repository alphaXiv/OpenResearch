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
import { BUTTON_CLASS_NAME, GHOST_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

const list = (paths: string[]) => paths.join(", ");

/** What the last sync did. A failure outranks everything below it, so the line
 * never reads "in step" above a red one saying otherwise; conflicts carry their
 * own rows. */
function status(overleaf: OverleafSync): string {
  if (overleaf.error) return "The last sync did not finish.";
  if (overleaf.syncing) return "Syncing with Overleaf…";
  if (overleaf.blocked) return "Save this file to sync it with Overleaf.";
  const last = overleaf.last;
  if (!last) return "This paper stays in step with Overleaf.";
  const moved: string[] = [];
  if (last.pulled.length) moved.push(`pulled ${list(last.pulled)}`);
  if (last.pushed.length) moved.push(`pushed ${list(last.pushed)}`);
  if (!moved.length) return last.conflicts.length ? "Nothing could be synced." : "In step with Overleaf.";
  const sentence = moved.join(", ");
  return `${sentence[0].toUpperCase()}${sentence.slice(1)}.`;
}

function UploadLink({ href }: { href: string }) {
  return (
    <a className="text-sm text-subtext whitespace-nowrap" href={href} target="_blank" rel="noreferrer">
      Upload a copy as a new project ↗
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
            Open in Overleaf <ExternalLink size={11} />
          </a>
          <button
            className={BUTTON_CLASS_NAME}
            disabled={overleaf.syncing || overleaf.blocked}
            data-tip={overleaf.blocked ? "Save the file first" : undefined}
            onClick={() => overleaf.sync()}
          >
            Sync now
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
            Unlink
          </button>
        </div>
        {conflicts.map((path) => (
          <div key={path} className="flex items-center flex-wrap gap-2 text-sm text-accent-red">
            <span className="flex-1 min-w-0">
              <code className="font-mono">{path}</code> changed here and on Overleaf. Both copies
              are untouched — choose which one to keep.
            </span>
            <button
              className={BUTTON_CLASS_NAME}
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "keep-local" })}
            >
              Keep this copy
            </button>
            <button
              className={BUTTON_CLASS_NAME}
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "take-overleaf" })}
            >
              Use Overleaf&apos;s
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
            Replace the Overleaf token
          </button>
        </div>
      </div>
    );
  }

  return (
    <form className="flex flex-col gap-1.5" onSubmit={submit}>
      <div className="text-sm text-subtext">
        {needsToken
          ? "Paste an Overleaf Git authentication token to keep this paper in step with an Overleaf project. Create one in Overleaf under Account Settings — Git integration comes with a paid Overleaf plan."
          : "Paste the URL of the Overleaf project this paper belongs to. Overleaf cannot create one over Git, so open or create the project there first."}
      </div>
      <div className="flex items-center flex-wrap gap-2">
        <input
          className="flex-1 min-w-55 font-mono text-sm"
          type={needsToken ? "password" : "text"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={needsToken ? "Overleaf Git token" : "https://www.overleaf.com/project/…"}
          autoComplete="off"
        />
        <button type="submit" className={BUTTON_CLASS_NAME} disabled={busy || !value.trim()}>
          {busy ? (needsToken ? "Saving…" : "Checking…") : needsToken ? "Save token" : "Link and sync"}
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
          {needsToken ? "Create a token ↗" : "My projects ↗"}
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
            Cancel
          </button>
        ) : (
          // A token can be rejected before this paper is ever linked, so the
          // way to replace it has to live in this state too.
          overleaf.hasToken && (
            <button type="button" className={GHOST_BUTTON_CLASS_NAME} onClick={replaceToken}>
              Replace the Overleaf token
            </button>
          )
        )}
      </div>
    </form>
  );
}
