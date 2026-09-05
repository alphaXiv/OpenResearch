import { m } from "../paraglide/messages.js";
// The shape follows what Overleaf allows: syncing needs a Git authentication
// token and a project that already exists (its bridge cannot create one), and
// the bridge is a paid feature. So the upload route — which creates a new
// project and works on any account — is offered in every state, not only after
// a refusal.

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { CloudUpload, ExternalLink, X } from "lucide-react";
import type { OverleafSync } from "../useOverleafSync";
import { getLocale } from "../paraglide/runtime.js";
import { autoDir, ltr } from "../i18n";
import { Button, ButtonLink, IconButton, Input, showAlert, Spinner } from "./ui";
import { usePopover } from "./ModelPicker";

export function OverleafButton({ overleaf }: { overleaf: OverleafSync }) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const { open, setOpen, ref } = usePopover(triggerRef);
  const id = useId();
  const [position, setPosition] = useState({ top: 0, left: 0, maxHeight: 0 });
  const conflicts = overleaf.last?.conflicts.length ?? 0;
  const failed = overleaf.hasToken && Boolean(overleaf.error);
  const tip = failed ? m.overleaf_sync_failed()
    : conflicts ? m.overleaf_conflict_tip()
    : !overleaf.hasToken || !overleaf.link ? m.overleaf_send_paper()
    : overleaf.syncing ? m.overleaf_syncing()
    : overleaf.blocked ? m.overleaf_save_to_sync()
    : m.overleaf_in_sync();

  useEffect(() => {
    if (conflicts) setOpen(true);
  }, [conflicts, setOpen]);

  useEffect(() => {
    if (failed) showAlert(m.overleaf_sync_failed(), "error", { id, duration: 5000 });
  }, [failed, overleaf.error, id]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !ref.current) return;
    const anchor = triggerRef.current.getBoundingClientRect();
    const width = Math.min(384, window.innerWidth - 16);
    const top = Math.min(anchor.bottom + 6, window.innerHeight - 80);
    setPosition({ top, left: Math.max(8, Math.min(anchor.right - width, window.innerWidth - width - 8)), maxHeight: window.innerHeight - top - 8 });
    (ref.current.querySelector<HTMLElement>("input") ?? ref.current).focus();
    const close = () => setOpen(false);
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && !ref.current?.contains(event.target)) close();
    };
    window.addEventListener("resize", close);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open, ref, setOpen]);

  return (
    <>
      <IconButton
        ref={triggerRef}
        size="small"
        active={open}
        disabled={!overleaf.loaded}
        data-tip={tip}
        data-tip-align="end"
        aria-label={m.a11y_overleaf_status({ status: tip })}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen(!open)}
      >
        {overleaf.syncing ? <Spinner /> : <CloudUpload size={13} className={failed || conflicts ? "text-accent-red" : overleaf.hasToken && overleaf.link ? "text-accent-green" : undefined} />}
      </IconButton>
      {open && createPortal(
        <div ref={ref} id={id} role="dialog" aria-label={m.settings_page_overleaf()} tabIndex={-1}
          className="fixed z-100 w-96 max-w-[calc(100vw-1rem)] overflow-auto rounded-lg border border-border bg-background p-4 text-text shadow-popover"
          style={position}
          onBlur={(event) => {
            if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget) && !triggerRef.current?.contains(event.relatedTarget)) setOpen(false);
          }}
        >
          <div className="absolute end-3 top-3">
            <IconButton size="small" aria-label={m.file_viewer_dismiss_overleaf_message()} onClick={() => { setOpen(false); triggerRef.current?.focus(); }}><X size={13} /></IconButton>
          </div>
          <OverleafPanel overleaf={overleaf} />
        </div>, document.body,
      )}
    </>
  );
}

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

  if (overleaf.link && !needsToken) {
    const conflicts = overleaf.last?.conflicts ?? [];
    return (
      <div className="flex flex-col gap-3">
        <div className="space-y-1.5 pe-8" role={overleaf.error ? "alert" : "status"}>
          <div className={`flex items-center gap-2 text-sm font-medium ${overleaf.error ? "text-accent-red" : "text-text"}`}>
            {overleaf.syncing && <Spinner />}
            {overleaf.error ? m.overleaf_sync_failed() : status(overleaf)}
          </div>
          {overleaf.error && <p className="text-sm text-text whitespace-pre-wrap break-words">{overleaf.error}</p>}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant={overleaf.error ? "primary" : "default"}
            disabled={overleaf.syncing || overleaf.blocked}
            data-tip={overleaf.blocked ? m.overleaf_save_first() : undefined}
            onClick={() => overleaf.sync()}
          >
            {overleaf.error ? m.app_retry() : m.overleaf_panel_sync_now()}
          </Button>
          <ButtonLink
            variant="ghost"
            href={overleaf.link.url}
            target="_blank"
            rel="noreferrer"
          >
            {m.overleaf_panel_open_in_overleaf()} <ExternalLink size={12} />
          </ButtonLink>
        </div>
        {conflicts.map((path) => (
          <div key={path} className="space-y-2 text-sm">
            <p className="break-words text-accent-red">
              <code className="font-mono">{path}</code> {m.overleaf_panel_changed_here_and_on_overleaf_both_copies_are()}
            </p>
            <div className="flex flex-wrap gap-2">
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
          </div>
        ))}
        {overleaf.last?.note && (
          <div className="text-sm text-accent-amber">{overleaf.last.note}</div>
        )}
        {formError && (
          <div role="alert" className="text-sm text-accent-red whitespace-pre-wrap break-words">{formError}</div>
        )}
        <details className="border-t border-border pt-3">
          <summary className="cursor-pointer text-sm font-semibold text-text focus-visible:outline-2 focus-visible:outline-text"><span className="ms-2">{m.settings_page_settings()}</span></summary>
          <div className="mt-2 flex flex-col items-start gap-1">
            <Button variant="ghost" className="font-normal" type="button" onClick={replaceToken}>
              {m.overleaf_panel_replace_the_overleaf_token()}
            </Button>
            <ButtonLink variant="ghost" className="font-normal" href={overleaf.uploadUrl} target="_blank" rel="noreferrer">
              {m.overleaf_panel_upload_a_copy_as_a_new_project()}
            </ButtonLink>
            <Button variant="ghost" className="font-normal"
              disabled={overleaf.syncing}
              onClick={() =>
                void overleaf.unlink().catch((err: unknown) => {
                  setFormError(err instanceof Error ? err.message : String(err));
                })
              }
            >
              {m.overleaf_panel_unlink()}
            </Button>
          </div>
        </details>
      </div>
    );
  }

  return (
    <form className="flex flex-col gap-1.5" onSubmit={submit}>
      <div className="pe-8 text-sm text-subtext">
        {needsToken
          ? m.overleaf_token_instructions()
          : m.overleaf_url_instructions()}
      </div>
      <div className="flex items-center flex-wrap gap-2">
        <Input
          className="basis-full min-w-0"
          aria-label={needsToken ? m.overleaf_git_token() : m.overleaf_my_projects()}
          aria-invalid={Boolean(formError)}
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
      {(formError || overleaf.error) && <div role="alert" className="text-sm text-accent-red whitespace-pre-wrap break-words">{formError || overleaf.error}</div>}
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
