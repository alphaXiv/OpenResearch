import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Run, RunBannerLevel } from "../../api";
import { hostForRun } from "../../engineHosts";
import { m } from "../../paraglide/messages.js";
import { mutUpdateUiState } from "../../queries/settings";
import { runActive } from "../../runDisplay";
import { plainSentence } from "../../runMonitor/format";
import { runModel, statedFlag, type RunModel } from "../../runMonitor/model";
import { shortLabels, summarize } from "../../runMonitor/summary";
import type { ShownRun } from "../../runMonitor/types";
import type { TabOpenIntent } from "../../tabPreview";
import type { LogSeek } from "../../logWindow";
import { useWorkspace } from "../../workspace";
import { Button } from "../ui";
import type { RunLabel } from "./RunLine";
import { RunStrip } from "./RunStrip";
import { RunSummary } from "./RunSummary";
import { useRunMonitors } from "./useRunMonitor";

type OpenRun = (runId: string, intent: TabOpenIntent, seek?: LogSeek) => void;

/** How long an announcement stays in the live region: long enough to be read out, too short to be read later as current. */
const ANNOUNCED_MS = 2_000;

/** What a screen reader hears when a run's words, state or freshness change; a number refilling is never news. */
function useAnnouncement(models: RunModel[], names: string[] | null): string {
  const seen = useRef<Map<string, { tone: string; words: string; stale: string | null; waiting: string | null }> | null>(null);
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    const first = seen.current === null;
    const previous = seen.current ?? new Map();
    const news: string[] = [];
    for (const [index, model] of models.entries()) {
      const before = previous.get(model.run.id);
      const prefix = names ? `${names[index]}: ` : "";
      const stale = model.stale?.state ?? null;
      // A finished run's own words already say it finished.
      const change = model.sentence.key === "end:done" ? "" : `${model.status}. `;
      const text = plainSentence([model.sentence.text, !model.live && model.sentence.watcher ? model.eta : ""].filter(Boolean).join(" · "));
      if (first) continue;
      if (!before || before.tone !== model.tone) news.push(`${prefix}${change}${text}`);
      else if (before.words !== model.sentence.key || (before.stale && !stale)) news.push(prefix + text);
      if (model.stale && before?.stale !== stale) {
        const separator = /[.。۔।][\u2066-\u2069]*$/u.test(model.stale.why) ? " " : ". ";
        news.push(`${prefix}${model.stale.why}${separator}${model.stale.age}.`);
      }
      if (model.needsChoice && before?.waiting !== model.flag?.id) news.push(`${prefix}${m.chat_run_needs_attention()}.`);
    }
    seen.current = new Map(models.map((model) => [model.run.id, {
      tone: model.tone,
      words: model.sentence.key,
      stale: model.stale?.state ?? null,
      waiting: model.needsChoice ? model.flag?.id ?? null : null,
    }]));
    const left = [...previous.keys()].some((id) => !seen.current?.has(id));
    if (news.length > 0) setAnnouncement(news.join(" "));
    else if (left) setAnnouncement("");
  });
  useEffect(() => {
    if (!announcement) return;
    const timer = window.setTimeout(() => setAnnouncement(""), ANNOUNCED_MS);
    return () => window.clearTimeout(timer);
  }, [announcement]);
  return announcement;
}

/** The runs of one chat above its composer: one run as its line or strip, several as one summary that opens into a line each, each opening in place. */
export function RunBanner({ sessionId, agent, onOpenRun, onOpenRunPage }: {
  sessionId: string;
  agent: string;
  onOpenRun?: OpenRun;
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
}) {
  const workspace = useWorkspace();
  const { getUiStateQuery, getRunMonitorQuery, listChatShownRunsQuery, useEngineHosts } = workspace.queries;
  const list = useQuery(listChatShownRunsQuery());
  const remembered = useQuery(getUiStateQuery()).data?.runBanner ?? "strip";
  const hosts = useEngineHosts();
  const section = useRef<HTMLElement>(null);
  const refocus = useRef<string | null>(null);
  const [level, setLevel] = useState<RunBannerLevel | null>(null);
  const [opened, setOpened] = useState<{ id: string | null } | null>(null);
  const [listed, setListed] = useState<boolean | null>(null);
  const [now, setNow] = useState(Date.now);

  // In the order they were launched, so a sweep reads as the agent started it and a new run joins at the end.
  const runs = (list.data?.runs ?? []).filter((item) => item.sessionId === sessionId).sort((a, b) => a.run.createdAt - b.run.createdAt);
  const shown = list.data?.source === "shown";
  const single = runs.length === 1;
  const live = runs.some((item) => runActive(item.run.status));
  // A run waiting for the user opens its strip by itself, by the same pick of source as everything the banner shows.
  const waiting = runs.filter((item) => {
    const cached = shown && item.run.monitor ? workspace.queryClient.getQueryData(getRunMonitorQuery(item.run.id).queryKey) ?? null : null;
    const flag = statedFlag(item.run, cached);
    return flag?.state === "waiting";
  }).map((item) => item.run.id);
  const openId = opened ? opened.id : waiting[0] ?? null;
  const singleLevel = level ?? (waiting.length > 0 ? "strip" : remembered);
  // The remembered level says whether a summary opens to its rows.
  const rows = listed ?? (waiting.length > 0 || remembered === "strip");
  const isOpen = (id: string) => single ? singleLevel === "strip" : rows && openId === id;
  // A lone run reads its monitor at either level, so its line keeps the watcher's ticks; several read only the open row's.
  const reads = (run: Run) => shown && (single || isOpen(run.id));

  const retryAt = (item: ShownRun) => {
    const host = hostForRun(hosts, item.run);
    return host?.status === "backoff" ? host.nextRetryMs : null;
  };
  const monitors = useRunMonitors(runs.map((item) => item.run), reads);
  const models = runs.map((item, index) => {
    const read = shown ? monitors[index] : null;
    return runModel({
      item,
      monitor: read && read.status !== "loading" ? read.data : null,
      monitorFailed: read?.status === "error",
      shown,
      agent: item.run.chatSessionId === sessionId ? agent : m.chat_the_agent(),
      now,
      nextRetryMs: retryAt(item),
    });
  });
  const titles = runs.map((item) => item.experimentName ?? item.experimentSlug ?? item.run.experimentId);
  const named = shortLabels(titles);
  const { labels, context } = named;
  const announcement = useAnnouncement(models, single ? null : titles);

  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);

  const waitingKey = waiting.join(" ");
  useEffect(() => {
    if (!waitingKey) return;
    setLevel(null);
    setOpened(null);
    setListed(null);
  }, [waitingKey]);

  useLayoutEffect(() => {
    const id = refocus.current;
    if (!id) return;
    refocus.current = null;
    section.current?.querySelector<HTMLElement>(`[data-run-toggle="${CSS.escape(id)}"]`)?.focus();
  });

  if (runs.length === 0 && !list.isError) return null;

  function toggle(id: string) {
    refocus.current = id;
    if (!single) {
      setOpened({ id: isOpen(id) ? null : id });
      return;
    }
    const next = singleLevel === "strip" ? "line" : "strip";
    setLevel(next);
    mutUpdateUiState.run(workspace, { runBanner: next });
  }

  function toggleRows() {
    refocus.current = "*";
    setListed(!rows);
    setOpened({ id: null });
    mutUpdateUiState.run(workspace, { runBanner: rows ? "line" : "strip" });
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    const id = single ? runs[0].run.id : openId;
    const step = id && isOpen(id) ? () => toggle(id) : !single && rows ? toggleRows : null;
    if (!step) return;
    event.preventDefault();
    event.stopPropagation();
    step();
  }

  const strip = (model: RunModel, index: number, label?: RunLabel) => {
    const monitor = shown ? monitors[index] : null;
    return (
      <RunStrip
        model={model}
        open={isOpen(model.run.id)}
        monitor={monitor}
        agent={model.run.chatSessionId === sessionId ? agent : null}
        label={label}
        onToggle={() => toggle(model.run.id)}
        onOpenRun={onOpenRun}
        onOpenRunPage={onOpenRunPage}
      />
    );
  };

  return (
    <section ref={section} aria-label={m.run_banner_label()} onKeyDown={onKeyDown}>
      <div role="status" className="sr-only">{announcement}</div>
      {list.isError && (
        <div className="mb-2.5 flex min-h-9 items-center gap-2 rounded-lg border border-border bg-surface py-1 ps-3 pe-1.5 text-sm text-ink-subtext">
          <span className="min-w-0 flex-1">{m.run_banner_load_failed()}</span>
          <Button size="small" variant="ghost" onClick={() => void list.refetch()}><RefreshCw size={13} />{m.app_retry()}</Button>
        </div>
      )}
      {runs.length === 0 ? null : single ? (
        strip(models[0], 0)
      ) : (
        <div data-run-frame="" className="@container relative mb-2.5 overflow-hidden rounded-lg border border-border bg-surface text-sm text-text">
          <RunSummary models={models} context={context} {...summarize(models, named, runs[0].watch, now, rows)} open={rows} onToggle={toggleRows} />
          {rows && (
            <ul role="list" className="m-0 list-none p-0">
              {models.map((model, index) => {
                const label = { text: labels[index], title: titles[index] };
                return (
                  <li key={model.run.id} className="border-t border-border-variant">
                    {strip(model, index, label)}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
