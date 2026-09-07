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
import type { OverleafLiveStatus } from "../api";
import type { OverleafSync } from "../useOverleafSync";
import { getLocale } from "../paraglide/runtime.js";
import { autoDir, ltr } from "../i18n";
import { TokenForm } from "./GitTokenForm";
import { Button, Spinner } from "./ui";

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

function liveText(live: OverleafLiveStatus): string {
  if (live.state === "live") return m.overleaf_live_on();
  if (live.state === "connecting") return m.overleaf_live_connecting();
  return live.error ?? m.overleaf_live_stopped();
}

/** Whether the last sync has nothing of its own to report, so the action row
 * is free to say the channel is live instead. */
function quiet(overleaf: OverleafSync): boolean {
  if (overleaf.error || overleaf.syncing || overleaf.blocked) return false;
  const last = overleaf.last;
  return !last || (!last.pulled.length && !last.pushed.length && !last.conflicts.length);
}

function Dot({ className }: { className: string }) {
  return <span className={`inline-block w-2 h-2 rounded-full shrink-0 ${className}`} />;
}

/** The live channel's line, for the states the action row does not carry
 * itself: waiting, connecting, or stopped with the way back in. A channel
 * that is simply up rides beside the buttons instead (`hideState`), so the
 * panel does not spend a whole row saying so. */
function LiveRow({ overleaf, hideState }: { overleaf: OverleafSync; hideState: boolean }) {
  const live = overleaf.live;
  // Not asked for yet: the git sync has to bring the paper into step first,
  // and a conflict or a failure keeps it that way.
  if (!live) return hideState ? null : <div className="text-sm text-subtext">{m.overleaf_live_waiting()}</div>;
  // The cookie is the problem (expired, or for another site): the way back
  // in is a fresh one, not a retry with the same.
  const cookieRefused = live.state === "stopped" && !!live.error?.includes("session cookie");
  if (hideState && !live.note && !cookieRefused) return null;
  const dot =
    live.state === "live"
      ? "bg-accent-green"
      : live.state === "connecting"
        ? "bg-accent-amber"
        : "bg-accent-red";
  return (
    <div className="flex flex-col gap-1">
      {!hideState && (
        <div className={`flex items-center flex-wrap gap-2 text-sm ${live.error ? "text-accent-red" : "text-subtext"}`}>
          <Dot className={dot} />
          <span className="flex-1 min-w-0">{liveText(live)}</span>
          {live.state === "stopped" && !cookieRefused && (
            <Button onClick={overleaf.retryLive}>{m.overleaf_live_retry()}</Button>
          )}
        </div>
      )}
      {live.note && <div className="text-sm text-accent-amber">{live.note}</div>}
      {cookieRefused && <SessionForm overleaf={overleaf} />}
    </div>
  );
}

/** Paste the browser session cookie that opens the live channel. A line and
 * a button until asked for: the paper already syncs, and a password field on
 * every linked tab would nag. */
function SessionForm({ overleaf }: { overleaf: OverleafSync }) {
  const [open, setOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Import reads a signed-in browser's cookie; pasting is the fallback the
  // error points to, so a failed import opens the paste form.
  const runImport = () => {
    if (importing) return;
    setImporting(true);
    setError(null);
    void overleaf
      .importSession()
      .catch((e: unknown) => {
        setError(e instanceof Error ? e.message : String(e));
        setOpen(true);
      })
      .finally(() => setImporting(false));
  };

  if (!open) {
    return (
      <div className="flex items-center flex-wrap gap-2 text-sm text-subtext">
        <span className="flex-1 min-w-0">{m.overleaf_go_live_prompt()}</span>
        {importing && <Spinner />}
        <Button type="button" disabled={importing} onClick={runImport}>
          {m.overleaf_import_cookie()}
        </Button>
        <Button variant="ghost" type="button" disabled={importing} onClick={() => setOpen(true)}>
          {m.overleaf_add_cookie()}
        </Button>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-sm text-subtext">{m.overleaf_session_instructions()}</div>
      {error && <div className="text-sm text-accent-red whitespace-pre-wrap">{error}</div>}
      <TokenForm
        save={overleaf.saveSession}
        onSaved={() => setOpen(false)}
        placeholder={m.overleaf_session_cookie()}
        createHref={overleaf.link?.url ?? ""}
        createLabel={m.overleaf_open_overleaf()}
        onCancel={() => setOpen(false)}
      />
    </div>
  );
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
    // A channel that is simply up is a one-line fact; it shares the action
    // row. When a sync has news of its own — files moved, a conflict — that
    // takes the row and the channel drops to its own line.
    const liveInline =
      overleaf.hasSession && overleaf.live?.state === "live" && quiet(overleaf);
    return (
      <div className="flex flex-col gap-1.5">
        <div className="flex items-center flex-wrap gap-2 text-sm text-subtext">
          {/* A channel that is up says so here rather than on a row of its
              own, which would leave this one blank beside the buttons. */}
          <span className="flex-1 min-w-0 flex items-center gap-2">
            {liveInline && <Dot className="bg-accent-green" />}
            <span className="min-w-0">{liveInline ? m.overleaf_live_on() : status(overleaf)}</span>
          </span>
          {overleaf.syncing && <Spinner />}
          <a
            className="inline-flex items-center gap-1 text-sm text-subtext whitespace-nowrap"
            href={overleaf.link.url}
            target="_blank"
            rel="noreferrer"
          >
            {m.overleaf_panel_open_in_overleaf()}
            <ExternalLink size={11} />
          </a>
          <Button
            disabled={overleaf.syncing || overleaf.blocked}
            data-tip={overleaf.blocked ? m.overleaf_save_first() : m.overleaf_sync_files_tip()}
            onClick={() => overleaf.sync()}
          >
            {m.overleaf_panel_sync_now()}
          </Button>
          <Button variant="ghost"
            disabled={overleaf.syncing}
            onClick={() =>
              void overleaf.unlink().catch((err: unknown) => {
                setFormError(err instanceof Error ? err.message : String(err));
              })
            }
          >
            {m.overleaf_panel_unlink()}
          </Button>
          {/* A stale token is the usual reason a sync stops working, so the
              way to replace it appears when one has failed — not as a
              standing row on a paper that is syncing fine. */}
          {overleaf.error && (
            <Button variant="ghost" type="button" onClick={replaceToken}>
              {m.overleaf_panel_replace_the_overleaf_token()}
            </Button>
          )}
        </div>
        {overleaf.hasSession ? (
          <LiveRow overleaf={overleaf} hideState={liveInline} />
        ) : (
          <SessionForm overleaf={overleaf} />
        )}
        {conflicts.map((path) => (
          <div key={path} className="flex items-center flex-wrap gap-2 text-sm text-accent-red">
            <span className="flex-1 min-w-0">
              <code className="font-mono">{path}</code> {m.overleaf_panel_changed_here_and_on_overleaf_both_copies_are()}
            </span>
            <Button
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "keep-local" })}
            >
              {m.overleaf_panel_keep_this_copy()}
            </Button>
            <Button
              disabled={overleaf.syncing || overleaf.blocked}
              onClick={() => overleaf.sync({ [path]: "take-overleaf" })}
            >
              {m.overleaf_panel_use_overleaf_apos_s()}
            </Button>
          </div>
        ))}
        {overleaf.last?.note && (
          <div className="text-sm text-accent-amber">{overleaf.last.note}</div>
        )}
        {formError && (
          <div className="text-sm text-accent-red whitespace-pre-wrap">{formError}</div>
        )}
        {/* Neither the upload link nor the token form lives here: uploading
            makes a *new* project, and the token is machine-wide and belongs to
            Settings. Both are for a paper that has no project yet. */}
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
          className="flex-1 min-w-55 text-sm"
          type={needsToken ? "password" : "text"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={needsToken ? m.overleaf_git_token() : "https://www.overleaf.com/project/…"}
          autoComplete="off"
       />
        <Button type="submit" disabled={busy || !value.trim()}>
          {busy ? (needsToken ? m.common_saving() : m.common_checking()) : needsToken ? m.overleaf_save_token() : m.overleaf_link_and_sync()}
        </Button>
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
          <Button variant="ghost"
            type="button"

            onClick={() => setReplacingToken(false)}
          >
            {m.overleaf_panel_cancel()}
          </Button>
        ) : (
          // A token can be rejected before this paper is ever linked, so the
          // way to replace it has to live in this state too.
          overleaf.hasToken && (
            <Button variant="ghost" type="button" onClick={replaceToken}>
              {m.overleaf_panel_replace_the_overleaf_token()}
            </Button>
          )
        )}
      </div>
    </form>
  );
}
