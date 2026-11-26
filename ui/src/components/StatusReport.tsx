import { useQuery } from "@tanstack/react-query";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown, ChevronUp, Ellipsis, NotebookText, RefreshCw } from "lucide-react";
import { useCallback, useContext, useEffect, useId, useLayoutEffect, useReducer, useRef, useState, type KeyboardEvent } from "react";
import { timeAgo, type ChatReport, type ChatReportChoice, type Harness, type Run } from "../api";
import { autoDir, fmtClock } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutChooseChatReportModel, mutMarkChatReportRead, mutRefreshChatReport, mutSetChatReportEnabled } from "../queries/chat";
import { mutUpdateUiState } from "../queries/settings";
import type { TabOpenIntent } from "../tabPreview";
import { useWorkspace } from "../workspace";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { FileScopeContext, type OpenFile } from "./FileReference";
import { Md } from "./Md";
import { ModelPicker } from "./ModelPicker";
import { ResizeHandle } from "./ResizeHandle";
import { InStatus, useCitedRuns } from "./RunFigure";
import { Button, IconButton, MenuItem, showAlert, Spinner, Tooltip } from "./ui";
import { cn } from "./ui/cn";

/** Asks for the model a chat's status needs: its own model in one click, or one that can write it from the model picker. */
const MIN_HEIGHT = 80;
/** The open card's height the user dragged to, kept for every chat. */
const HEIGHT_KEY = "status-height";

/** The card body can grow, or restore a saved height, up to this height, then scrolls inside. */
function maxBodyHeight(dom?: HTMLElement | null): number {
  const pane = dom?.closest<HTMLElement>(".chat-pane");
  const composer = dom?.closest<HTMLElement>(".composer");
  const chrome = composer && dom ? Math.max(0, composer.offsetHeight - dom.offsetHeight) : 0;
  return Math.max(MIN_HEIGHT, Math.floor(((pane?.clientHeight || window.innerHeight) - chrome) * 0.8));
}

function ModelOffer({ sessionId, needs, onChoose, onOpenSettings }: {
  sessionId: string;
  needs: NonNullable<ChatReport["needsModel"]>;
  onChoose: (choice: ChatReportChoice) => void;
  onOpenSettings: () => void;
}) {
  const { listChatReportModelsQuery } = useWorkspace().queries;
  const routes = useQuery(listChatReportModelsQuery(sessionId));
  const writes = useCallback(
    (harness: Harness, model: string) => routes.data?.some((route) => route.harness === harness.id && route.profile === (harness.profile?.id ?? null) && route.model === model) ?? false,
    [routes.data],
  );
  const choosable = Boolean(routes.data?.length);
  const none = routes.data && !choosable && !needs.same;
  const trigger = needs.same
    ? <Button size="small" variant="primary" className="rounded-s-none border-s-plan-caret px-1.5" aria-label={m.status_report_other_model()}><ChevronDown size={14} aria-hidden="true" /></Button>
    : <Button size="small" variant="primary">{m.status_report_choose_model()}</Button>;

  return (
    <div className="flex min-w-0 flex-1 items-center gap-2">
      <span role={routes.isError ? "alert" : undefined} className="min-w-0 flex-1 text-ink-subtext">
        {routes.isError ? routes.error.message : none ? m.status_report_no_models() : m.status_report_offer()}
      </span>
      {routes.isError && (
        <Button size="small" variant="ghost" className="shrink-0" onClick={() => void routes.refetch()}>
          <RefreshCw size={13} />{m.app_retry()}
        </Button>
      )}
      {none && <Button size="small" variant="primary" className="shrink-0" onClick={onOpenSettings}>{m.status_report_set_up_agents()}</Button>}
      <div className="flex shrink-0">
        {needs.same && (
          <Button size="small" variant="primary" className={cn(choosable && "rounded-e-none")} onClick={() => onChoose({ same: true })}>
            {m.status_report_enable()}
          </Button>
        )}
        {choosable && (
          <ModelPicker
            value={null}
            onSelect={({ harness, profile, model }) => onChoose({ harness, profile: profile ?? null, model: model ?? "" })}
            onOpenSettings={onOpenSettings}
            only={writes}
            trigger={trigger}
          />
        )}
      </div>
    </div>
  );
}

/** What the agent has done and is doing, rewritten by the chat's small model at each milestone. */
export function StatusReport({ sessionId, onOpenFile, onOpenRunPage, onOpenSettings, onTurnOff }: {
  sessionId: string;
  /** The card leaves when the user turns it off, so focus needs a new home. */
  onTurnOff: () => void;
  /** Opens Settings → Agents, where the user sets up a model that can write the status. */
  onOpenSettings: () => void;
  onOpenFile?: OpenFile;
  /** Opens a run's experiment, where each cited result links. */
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
}) {
  const workspace = useWorkspace();
  const cited = useCitedRuns(useContext(FileScopeContext)?.projectId);
  const openExperiment = onOpenRunPage && ((runId: string, intent: TabOpenIntent) => {
    const { run } = cited(runId);
    if (run) onOpenRunPage(run, intent);
  });
  const { getChatReportQuery, getUiStateQuery } = workspace.queries;
  const query = useQuery(getChatReportQuery(sessionId));
  const forNewChats = useQuery(getUiStateQuery()).data?.statusForNewChats;
  // Folded until opened in this chat, and remembered only for this tab.
  const openKey = `status-open:${workspace.url("/api/")}:${sessionId}`;
  const [open, setOpen] = useState(() => {
    try {
      return sessionStorage.getItem(openKey) === "1";
    } catch {
      return false;
    }
  });
  const { portalContainer } = useContext(WorkspaceDomContext);
  const toggleButton = useRef<HTMLButtonElement>(null);
  const bodyBox = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(() => {
    try {
      return Number(localStorage.getItem(HEIGHT_KEY)) || null;
    } catch {
      return null;
    }
  });
  const [heightLimit, setHeightLimit] = useState(() => maxBodyHeight());
  const headlineId = useId();
  const report = query.data;
  const shown = !query.isError && report && report.enabled && (report.body !== null || report.writing || report.error !== null || report.needsModel !== null);
  const expanded = Boolean(shown && open && report.body !== null);

  /** A write the server refused, shown until the server reports something other than the status it was refused over. */
  const [refusal, setRefusal] = useState<{ message: string; over: string; pick: boolean } | null>(null);
  const refusedOver = (over: ChatReport, pick: boolean) => ({
    onError: (error: unknown) => setRefusal({ message: error instanceof Error ? error.message : String(error), over: JSON.stringify(over), pick }),
  });

  // The updatedAt from before the latest write, so a failed write isn't announced as an update.
  const [writeFrom, setWriteFrom] = useState<number | null | undefined>(undefined);
  if (report && (writeFrom === undefined || (report.writing && writeFrom !== report.updatedAt))) setWriteFrom(report.updatedAt);

  const [, rerender] = useReducer((tick: number) => tick + 1, 0);
  useEffect(() => {
    if (!shown) return;
    const timer = window.setInterval(rerender, 30_000);
    return () => window.clearInterval(timer);
  }, [shown]);

  useLayoutEffect(() => {
    if (!expanded) return;
    const dom = bodyBox.current;
    if (!dom) return;
    const measure = () => setHeightLimit(maxBodyHeight(dom));
    measure();
    const pane = dom.closest(".chat-pane");
    const composer = dom.closest(".composer");
    const observer = new ResizeObserver(measure);
    if (pane) observer.observe(pane);
    if (composer) observer.observe(composer);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [expanded]);

  const frame = "mb-2.5 overflow-hidden rounded-lg border border-border bg-surface text-sm text-text";
  if (query.isError) {
    return (
      <section aria-label={m.status_report_label()} className={cn(frame, "flex min-h-9 items-center gap-2 py-1 ps-3 pe-1.5 text-ink-subtext")}>
        <span role="alert" className="min-w-0 flex-1">{m.status_report_load_failed()}</span>
        <Button size="small" variant="ghost" onClick={() => void query.refetch()}><RefreshCw size={13} />{m.app_retry()}</Button>
      </section>
    );
  }
  if (!shown) return null;

  const body = report.body?.trim() ?? "";
  const lineEnd = body.indexOf("\n");
  const headline = (lineEnd < 0 ? body : body.slice(0, lineEnd)).replace(/^#+\s+/, "");
  const rest = lineEnd < 0 ? "" : body.slice(lineEnd + 1).trim();
  const written = report.body !== null;
  const needs = report.writing ? null : report.needsModel;
  const version = report.version;
  const folded = written && !expanded && !needs;
  const renderedHeight = height === null ? undefined : Math.min(Math.max(height, MIN_HEIGHT), heightLimit);
  // By value: the rollback after a refusal restores an equal copy, not the same object.
  const refused = refusal?.over === JSON.stringify(report) ? refusal : null;
  const failure = refused?.message ?? (report.writing ? null : report.error?.message ?? null);
  const updated = report.updatedAt !== null && (writeFrom == null || report.updatedAt > writeFrom);
  const toggleLabel = expanded ? m.common_show_less() : m.common_show_more();
  function toggle() {
    setOpen(!open);
    try {
      if (open) sessionStorage.removeItem(openKey);
      else sessionStorage.setItem(openKey, "1");
    } catch {}
  }

  function resize(next: number | null) {
    setHeight(next);
    try {
      if (next === null) localStorage.removeItem(HEIGHT_KEY);
      else localStorage.setItem(HEIGHT_KEY, String(next));
    } catch {}
  }

  function markRead(version: number) {
    mutMarkChatReportRead.run(workspace, sessionId, version);
    if (open) toggle();
    toggleButton.current?.focus();
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "Escape" || event.defaultPrevented || !expanded) return;
    event.preventDefault();
    event.stopPropagation();
    toggleButton.current?.focus();
    toggle();
  }

  return (
    <InStatus value>
      <section aria-label={m.status_report_label()} className={cn(frame, "status-report relative")} onKeyDown={onKeyDown}>
        {expanded && (
          <ResizeHandle
            label={m.status_report_resize()}
            height={height}
            min={MIN_HEIGHT}
            max={heightLimit}
            measure={() => bodyBox.current?.offsetHeight ?? MIN_HEIGHT}
            onChange={resize}
          />
        )}
        <div role="status" className="sr-only">{report.writing ? m.status_report_updating() : updated ? m.status_report_updated() : ""}</div>
        <div className="flex min-h-9 min-w-0 items-center gap-x-2 py-1 ps-3 pe-1.5">
          <span aria-hidden="true" className="flex h-7 shrink-0 items-center text-ink-subtext">
            {report.writing ? <Spinner /> : <NotebookText size={14} />}
          </span>
          {/* The chevron is the keyboard control; the folded line is inert so its links can't take the click that opens the card. */}
          <div className={cn("flex min-w-0 flex-1 items-center self-stretch", folded && "cursor-pointer")} onClick={folded ? toggle : undefined}>
            {needs ? (
              <ModelOffer
                sessionId={sessionId}
                needs={needs}
                onChoose={(choice) => mutChooseChatReportModel.runWithOptions(workspace, sessionId, choice, refusedOver(report, true))}
                onOpenSettings={onOpenSettings}
              />
            ) : !written ? (
              <span className="block min-w-0 truncate text-ink-subtext">{report.writing ? m.status_report_first_writing() : m.status_report_none()}</span>
            ) : !expanded && (
              <div id={headlineId} inert className={cn("min-w-0 flex-1 [&_.md]:leading-normal [&_p]:my-0 [&_p]:truncate", report.read && "[&_.md]:text-ink-subtext [&_strong]:text-ink-subtext")}>
                <Md text={headline} />
              </div>
            )}
          </div>
          {report.updatedAt !== null && (
            <span className="shrink-0 whitespace-nowrap tabular-nums text-ink-subtext">
              {[fmtClock(report.updatedAt), timeAgo(report.updatedAt)].map(autoDir).join(" · ")}
            </span>
          )}
          {written && !report.read && !needs && typeof version === "number" && (
            <Button size="small" variant="ghost" className="shrink-0" onClick={() => markRead(version)}>
              <Check size={13} aria-hidden="true" />{m.status_report_mark_read()}
            </Button>
          )}
          <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
            <Menu.Root modal={false}>
              <Tooltip interactive content={m.status_report_options()}>
                <Menu.Trigger render={<IconButton size="small" aria-label={m.status_report_options()} />}>
                  <Ellipsis size={14} aria-hidden="true" />
                </Menu.Trigger>
              </Tooltip>
              <Menu.Portal container={portalContainer}>
                <Menu.Positioner className="z-70" positionMethod="fixed" side="top" align="end" sideOffset={4} collisionPadding={8}>
                  <Menu.Popup className="option-menu min-w-44 max-h-(--available-height) overflow-y-auto rounded-md border border-border bg-background p-1 shadow-menu outline-none">
                    <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={() => {
                        mutSetChatReportEnabled.runWithOptions(workspace, sessionId, false, { onSuccessUi: () => showAlert(m.status_report_turned_off(), "info") });
                        onTurnOff();
                      }}>
                      {m.status_report_turn_off()}
                    </Menu.Item>
                    {forNewChats && (
                      <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={() => {
                        mutUpdateUiState.runWithOptions(workspace, { statusForNewChats: false }, { onSuccessUi: () => showAlert(m.status_report_turned_off_new(), "info") });
                      }}>
                        {m.status_report_turn_off_new()}
                      </Menu.Item>
                    )}
                  </Menu.Popup>
                </Menu.Positioner>
              </Menu.Portal>
            </Menu.Root>
          </DirectionProvider>
          {written && (
            <Tooltip interactive content={toggleLabel}>
              <IconButton ref={toggleButton} size="small" aria-label={toggleLabel} aria-expanded={expanded} aria-describedby={expanded ? undefined : headlineId} onClick={toggle}>
                {expanded ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
              </IconButton>
            </Tooltip>
          )}
        </div>
        {failure && (
          <div className="flex min-h-9 items-center gap-2 py-1 ps-3 pe-1.5 text-xs text-ink-subtext">
            <span role="alert" className="min-w-0 flex-1">{failure}</span>
            {/* A refused pick is retried from the offer above. */}
            {!refused?.pick && (
              <Button size="small" variant="ghost" disabled={report.writing} onClick={() => mutRefreshChatReport.runWithOptions(workspace, sessionId, refusedOver(report, false))}>
                <RefreshCw size={13} />{m.app_retry()}
              </Button>
            )}
          </div>
        )}
        {/* Figures draw compact (a chart about 200px with its title and frame, images at most 192px), so one fits whole beside some text. */}
        {expanded && (
          <div
            ref={bodyBox}
            className={cn("overflow-y-auto overscroll-y-contain border-t border-border-variant px-3 py-2 [&_img]:max-h-48 [&_img]:w-auto", height === null ? "max-h-[50vh]" : "max-h-[80vh]")}
            style={{ minHeight: MIN_HEIGHT, height: renderedHeight, maxHeight: height !== null ? heightLimit : undefined }}
          >
            <Md text={rest ? `${headline}\n\n${rest}` : headline} onOpenFile={onOpenFile} onOpenRun={openExperiment} />
          </div>
        )}
      </section>
    </InStatus>
  );
}
