import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";

import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { RefreshCw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { type Changes, type UpdateStatus } from "../api";
import { getLocale } from "../paraglide/runtime.js";

import { Button, IconButton, Spinner } from "./ui";

export interface UpdateState {
  status: UpdateStatus | null;
  /** The initial fetch failed — distinct from "still loading". */
  error: string | null;
  /** Adopt a status returned by a mutating call, so the card reflects the write
   *  immediately instead of waiting for the next SSE sample. */
  apply: (status: UpdateStatus) => void;
}

/** Live update status: the initial fetch plus every `update.status` SSE frame.
 *  Exported because the Updates settings card renders the same state. */
export function useUpdateStatus(enabled = true): UpdateState {
  const { getUpdateStatusQuery } = useWorkspace().queries;
  const { setScopedQueryData } = useWorkspace();

  const options = getUpdateStatusQuery();
  const query = useQuery({ ...options, enabled });
  return {
    status: query.data ?? null,
    error: query.error?.message ?? null,
    apply: (status) => setScopedQueryData(options.queryKey, status),
  };
}

/** How long to wait for the relaunched server before giving up on the reload. It may
 *  download and install a held-back update first, which the server bounds at 5 minutes. */
const RESTART_TIMEOUT_MS = 360_000;
const RESTART_POLL_MS = 500;

export interface RestartState {
  restarting: boolean;
  error: string | null;
  restart: () => void;
}

/** "3 new features, 2 bug fixes, and 5 other changes", leaving out empty groups; null when nothing changed. */
export function describeChanges(changes: Changes | null): string | null {
  if (!changes) return null;
  const locale = getLocale();
  const plural = new Intl.PluralRules(locale);
  const groups = [
    [changes.features, m.update_changes_features_one, m.update_changes_features_other],
    [changes.fixes, m.update_changes_fixes_one, m.update_changes_fixes_other],
    [changes.other, m.update_changes_other_one, m.update_changes_other_other],
  ] as const;
  const parts = groups.flatMap(([count, one, other]) => (count ? [(plural.select(count) === "one" ? one : other)({ count })] : []));
  return parts.length ? new Intl.ListFormat(locale).format(parts) : null;
}

export const releaseNotesUrl = (tag: string) =>
  `https://github.com/alphaXiv/OpenResearch/releases/tag/${tag.startsWith("canary-") ? "canary" : tag.split("/").map(encodeURIComponent).join("/")}`;

/** Ask the server to relaunch, then reload once a different server process
 *  answers — the page itself is the old build until it reloads. The old server answers the POST and then drops every connection,
 *  so the polling errors in between are expected and swallowed. */
export function useRestartApp(status: UpdateStatus | null): RestartState {
  const { getUpdateStatusQuery } = useWorkspace().queries;
  const { isCurrentScope, queryClient } = useWorkspace();
  const { restartApp } = useWorkspace().api;

  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const unmounted = useRef(false);
  useEffect(() => {
    unmounted.current = false;
    return () => {
      unmounted.current = true;
    };
  }, []);

  const previous = status?.restartRequired ? status.instance : null;
  const restart = () => {
    if (!previous || restarting) return;
    const options = getUpdateStatusQuery();
    const retired = () => unmounted.current || !isCurrentScope(options.queryKey);
    setRestarting(true);
    setError(null);
    void (async () => {
      try {
        await restartApp();
        if (retired()) return;
        const deadline = Date.now() + RESTART_TIMEOUT_MS;
        while (Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, RESTART_POLL_MS));
          if (retired()) return;
          const next = await queryClient.fetchQuery({ ...options, staleTime: 0 }).catch(() => null);
          if (retired()) return;
          if (next && next.instance !== previous) {
            window.location.reload();
            return;
          }
        }
        throw new Error(m.update_banner_restart_timed_out());
      } catch (e) {
        if (retired()) return;
        setError(e instanceof Error ? e.message : String(e));
        setRestarting(false);
      }
    })();
  };
  return { restarting, error, restart };
}

/** Shown once a newer version is installed, or held back until this server
 *  restarts: the app the user is looking at is the old one until it restarts.
 *
 *  Deliberately not shown for a merely *available* update — that is the
 *  updater's job, and a banner for something already in hand is noise. */
export function UpdateBanner({ status, hasSidebar = false }: { status: UpdateStatus | null; hasSidebar?: boolean }) {
  const [dismissed, setDismissed] = useState<string | null>(null);

  // `installedVersion`, not `latest`: a release can land between the install and
  // the restart, and the banner must name what is actually on disk.
  const version = status?.restartRequired ? status.installedVersion : null;
  const { restarting, error, restart } = useRestartApp(status);
  if (!version || dismissed === version) return null;

  const canary = status?.installedTag?.startsWith("canary-");
  const changes = describeChanges(status?.changes ?? null);
  const label = ltr(version.startsWith("v") ? version : `v${version}`);

  return (
    <div
      className={`update-banner flex items-center gap-2 shrink-0 min-h-11 py-1.5 px-3.5 ${hasSidebar ? "mac-titlebar:ps-titlebar-sidebar-controls" : "mac-titlebar:ps-titlebar-controls"} mac-titlebar:min-h-titlebar mac-titlebar:pt-px mac-titlebar:pb-0 win-titlebar:pe-win-controls text-sm text-text bg-accent-blue-subtle border-b border-b-accent-blue-border`}
      role="status"
    >
      <RefreshCw size={14} className={`shrink-0 text-accent-blue-strong${restarting ? " animate-spin" : ""}`} />
      <span className="min-w-0 truncate" title={error ? m.update_banner_restart_failed({ error }) : undefined}>
        {error
          ? m.update_banner_restart_failed({ error })
          : canary
          ? changes ? m.update_banner_canary_changes({ changes }) : m.update_banner_canary_ready()
          : changes ? m.update_banner_changes({ version: label, changes }) : m.update_banner_complete({ version: label })}
      </span>
      <div className="ms-auto flex shrink-0 items-center gap-2">
        {status?.installedTag && (
          <a href={releaseNotesUrl(status.installedTag)} target="_blank" rel="noreferrer" className="text-sm text-subtext underline">
            {m.settings_release_notes()}
          </a>
        )}
        {status?.canRestart && (
          <Button type="button" size="small" disabled={restarting} onClick={restart}>
            {restarting ? m.update_banner_restarting() : m.update_banner_restart()}
          </Button>
        )}
        <IconButton
          type="button"
          size="small"
          aria-label={m.update_banner_dismiss()}
          disabled={restarting}
          onClick={() => setDismissed(version)}
        >
          <X size={14} />
        </IconButton>
      </div>
    </div>
  );
}

const COMMIT_TITLE = /^(\w+)(?:\(([^)]*)\))?!?:\s*(.*)$/;

/** The commits a canary build adds over stable, grouped the way the banner counts them. */
export function BuildNotes() {
  const { getBuildNotesQuery } = useWorkspace().queries;
  const { data, error, isPending } = useQuery(getBuildNotesQuery());
  if (isPending) return <div className="flex items-center gap-2 text-subtext"><Spinner /> {m.settings_page_loading()}</div>;
  if (error) return <p role="alert" className="m-0 text-accent-red">{m.update_notes_failed({ error: error.message })}</p>;
  if (!data) return <p className="m-0 text-subtext">{m.update_notes_unavailable()}</p>;

  const groups = { features: [], fixes: [], other: [] } as Record<"features" | "fixes" | "other", { scope?: string; text: string }[]>;
  for (const title of data.commits) {
    const parsed = COMMIT_TITLE.exec(title);
    const group = parsed?.[1] === "feat" ? "features" : parsed?.[1] === "fix" ? "fixes" : "other";
    groups[group].push(parsed && group !== "other" ? { scope: parsed[2], text: parsed[3] } : { text: title });
  }
  const sections = [
    [groups.features, m.update_notes_features()],
    [groups.fixes, m.update_notes_fixes()],
    [groups.other, m.update_notes_other()],
  ] as const;

  return (
    <div className="flex flex-col gap-4 text-sm">
      <p className="m-0 text-subtext">
        {data.commits.length ? m.update_notes_since({ base: ltr(data.base) }) : m.update_notes_empty({ base: ltr(data.base) })}
      </p>
      {sections.map(([items, heading]) => items.length > 0 && (
        <section key={heading}>
          <h3 className="m-0 mb-1.5 text-sm font-medium text-text">{heading}</h3>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {items.map((item, index) => (
              <li key={index} dir="auto" className="text-text">
                {item.scope && <span className="text-subtext">{item.scope}: </span>}
                {item.text}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
