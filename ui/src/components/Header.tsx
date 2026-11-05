import { useContext, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Popover } from "@base-ui/react/popover";
import { formatForDisplay } from "@tanstack/react-hotkeys";
import { PanelLeft } from "lucide-react";

import { appHotkeys } from "../hotkeys";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { useRuntime } from "../RemoteRuntime";
import { setQueryDevtoolsOpen, useQueryDevtoolsOpen } from "./QueryDevtools";
import { BuildNotes, describeChanges, releaseNotesUrl, useRestartApp, useUpdateStatus } from "./UpdateBanner";
import { BrandMark } from "./Wordmark";
import { Badge, Button, IconButton, Switch, Tooltip } from "./ui";

/** The same box whether the sidebar is open or closed, so the toggle never moves under the pointer. */
export function SidebarToggle({ open, onClick, className }: { open: boolean; onClick: () => void; className?: string }) {
  const label = open ? m.header_hide_sidebar() : m.chat_panel_show_sidebar();
  return (
    <Tooltip interactive content={`${label} (${formatForDisplay(appHotkeys["sidebar.toggle"].hotkey ?? [])})`} className={`sidebar-toggle rounded-md ${className ?? ""}`}>
      <IconButton aria-label={label} aria-expanded={open} onClick={onClick}>
        <PanelLeft size={18} />
      </IconButton>
    </Tooltip>
  );
}

export function RailHeader({ onCollapse }: { onCollapse?: () => void }) {
  return (
    <div className="rail-brand mac-titlebar-spacer flex h-12 shrink-0 items-center gap-1.5 ps-2.5 pe-2 max-md:ps-1 mac-titlebar:h-titlebar mac-titlebar:ps-[calc(var(--spacing-titlebar-toggle)+2.375rem)]">
      {/* The macOS app keeps one toggle beside the traffic lights for both states. */}
      {onCollapse && <SidebarToggle open onClick={onCollapse} className="mac-titlebar:hidden" />}
      <Brand />
    </div>
  );
}

/** The logo and name, marked when this isn't a stable release; hovering shows the version and its update state. */
function Brand() {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const { applyUpdate } = useWorkspace().api;
  const { showFormDialog } = useWorkspace().dialogs;

  const runtime = useRuntime();
  const { status, apply } = useUpdateStatus(runtime.kind === "local");
  const restart = useRestartApp(status);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canary = status?.current.startsWith("canary-") ?? false;
  const dev = !canary && runtime.buildChannel === "development";
  const badge = canary ? m.settings_update_channel_canary() : dev ? m.header_dev_badge() : null;

  const check = () => {
    setChecking(true);
    setError(null);
    applyUpdate()
      .then(apply, (e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setChecking(false));
  };
  const busy = checking || restart.restarting;
  const action = !status ? null
    : status.restartRequired && status.canRestart ? { label: restart.restarting ? m.update_banner_restarting() : m.update_banner_restart(), run: restart.restart }
      : status.selfUpdates ? { label: checking ? m.chat_working() : status.updateAvailable ? m.settings_update_now() : m.settings_check_now(), run: check }
        : null;
  const currentTag = status && !canary && !dev ? `v${status.current}` : null;
  const changes = status && (status.updateAvailable || status.restartRequired) ? describeChanges(status.changes) : null;
  const shownError = restart.error ? m.update_banner_restart_failed({ error: restart.error }) : error;

  return (
    <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Popover.Root>
        <Popover.Trigger
          openOnHover
          delay={300}
          closeDelay={150}
          type="button"
          aria-label={badge ? `OpenResearch ${badge}` : "OpenResearch"}
          className="flex min-w-0 items-center gap-1.5 rounded-sm text-sm font-medium text-text outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
        >
          <span className="size-4.5 shrink-0"><BrandMark fill={dev ? "var(--accent-orange)" : undefined} /></span>
          {/* The name wraps onto a clipped second line when it can't fit whole, leaving the logo and badge. */}
          <span className="flex h-5 min-w-0 flex-row-reverse flex-wrap content-start items-center justify-end overflow-hidden">
            {badge ? <Badge size="small" className="shrink-0">{badge}</Badge> : <span />}
            <span className={`whitespace-nowrap ${badge ? "me-1.5" : ""}`}>OpenResearch</span>
          </span>
        </Popover.Trigger>
        <Popover.Portal container={portalContainer}>
          <Popover.Positioner side="bottom" align="start" sideOffset={6} collisionPadding={8} className="z-50">
            <Popover.Popup aria-label="OpenResearch" className="w-max min-w-56 max-w-[calc(100vw-1rem)] rounded-lg border border-border bg-background p-3 text-sm text-text shadow-menu outline-none">
              <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-4 gap-y-2">
                <dt className="text-subtext">{m.settings_page_version()}</dt>
                <dd className="m-0 flex items-center justify-between gap-3">
                  <span className="truncate">{ltr(status?.current ?? runtime.version)}</span>
                  {canary
                    ? <button type="button" className="shrink-0 cursor-pointer text-subtext underline" onClick={() => showFormDialog(m.update_notes_title(), () => <BuildNotes />)}>{m.update_notes_open()}</button>
                    : currentTag && <ReleaseNotesLink tag={currentTag} />}
                </dd>
                {status && <>
                  <dt className="text-subtext">{m.settings_page_latest()}</dt>
                  <dd className="m-0 flex items-center justify-between gap-3">
                    <span className="truncate">{ltr(status.latest ?? "—")}</span>
                    {status.updateAvailable && status.latestTag && <ReleaseNotesLink tag={status.latestTag} />}
                    {action && <Button size="small" disabled={busy} onClick={action.run}>{action.label}</Button>}
                  </dd>
                </>}
              </dl>
              {changes && <p className="mt-2 mb-0 text-subtext">{changes}</p>}
              {shownError && <p role="alert" className="mt-2 mb-0 text-accent-red">{shownError}</p>}
              {import.meta.env.DEV && <DevtoolsSwitch />}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </DirectionProvider>
  );
}

function ReleaseNotesLink({ tag }: { tag: string }) {
  return (
    <a href={releaseNotesUrl(tag)} target="_blank" rel="noreferrer" className="shrink-0 text-subtext underline">
      {m.settings_release_notes()}
    </a>
  );
}

function DevtoolsSwitch() {
  const open = useQueryDevtoolsOpen();
  const label = m.header_query_devtools();
  return (
    <div className="mt-3 flex items-center justify-between gap-3 border-t border-border pt-3">
      <span className="text-subtext">{label}</span>
      <Switch checked={open} aria-label={label} onClick={() => setQueryDevtoolsOpen(!open)} />
    </div>
  );
}
