import { useQuery } from "@tanstack/react-query";
import { ChartSpline, ChevronRight, FlaskConical, NotebookText } from "lucide-react";
import { useContext, useId, useState } from "react";
import { fmtDuration, fmtNumber, type TrackioMetric } from "../api";
import { autoDir, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { familyName, headlineMetric, paperHeader, type CitedPaper, type PaperMeta } from "../researchActivity";
import { runActive, runDisplay } from "../runDisplay";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { useWorkspace } from "../workspace";
import { FileScopeContext } from "./FileReference";
import { LitSourceLogo, paperUrl, type LitSource } from "./LitSourceLogo";
import { StatusBadge } from "./StatusBadge";

/** The message for `count` in the reader's language: its `_one` or `_other` key. */
export function pluralMessage<T>(count: number, one: (params: T) => string, other: (params: T) => string) {
  return new Intl.PluralRules(getLocale()).select(count) === "one" ? one : other;
}

const fmtYear = (year: number) => new Intl.NumberFormat(getLocale(), { useGrouping: false }).format(year);

export interface ShelfPaper {
  source: LitSource;
  id: string;
  /** What `orx paper` printed, whose header names an OpenAlex, bioRxiv or PubMed paper. */
  output?: string;
  /** The calls that read it, which chat search jumps to. */
  partIds?: string[];
}

/** Anchors for the tool calls a card stands for, so chat search can scroll to each. */
function PartAnchors({ ids = [] }: { ids?: string[] }) {
  return ids.map((id) => <span key={id} className="absolute inset-x-0 top-0" data-part-id={id} />);
}

function byline(authors: string[]): string {
  const [first, second] = authors.map(familyName);
  if (authors.length === 1) return first;
  if (authors.length === 2) return m.paper_authors_two({ first, second });
  return m.paper_authors_et_al({ author: first });
}

function PaperCard({ paper, discovered, note, oneLine = false }: {
  paper: ShelfPaper;
  discovered?: PaperMeta;
  /** Said after the byline, such as the pages the answer cites. */
  note?: string;
  oneLine?: boolean;
}) {
  const { resolvePaperQuery } = useWorkspace().queries;
  const resolved = useQuery({ ...resolvePaperQuery(paper.id), enabled: paper.source === "alphaxiv" }).data;
  const header = paper.output ? paperHeader(paper.output) : null;
  const title = resolved?.title?.replace(/\s+/g, " ").trim() || header?.title || discovered?.title || paper.id;
  const authors = resolved?.authors?.length ? resolved.authors : header?.authors;
  const year = resolved?.publishedAt ? new Date(resolved.publishedAt).getUTCFullYear() : header?.year ?? discovered?.year;
  const byLine = authors?.length ? byline(authors) : year || note ? null : ltr(paper.id);
  const rest = [year ? fmtYear(year) : null, note].filter(Boolean).join(" · ");

  return (
    <a
      className="paper-card relative flex min-w-0 gap-2.5 no-underline rounded-lg border border-border-variant bg-surface px-2.5 py-2 hover:border-border focus-visible:outline-2 focus-visible:outline-focus"
      href={paperUrl(paper.source, paper.id)}
      target="_blank"
      rel="noopener noreferrer"
    >
      <PartAnchors ids={paper.partIds} />
      <LitSourceLogo source={paper.source} size={16} className="mt-0.5" />
      <span className="min-w-0">
        <span className={`block text-sm leading-5 text-text ${oneLine ? "truncate" : "line-clamp-2"}`} dir="auto">{title}</span>
        <span className="mt-0.5 flex min-w-0 text-xs leading-4 text-muted">
          {byLine && <span className="block max-w-[70%] shrink-0 truncate" dir="auto">{byLine}</span>}
          {rest && <span className="min-w-0 truncate whitespace-pre">{byLine ? ` · ${rest}` : rest}</span>}
        </span>
      </span>
    </a>
  );
}

/** The papers a turn read, as cards that open each paper on its source. */
export function PaperShelf({ papers, discovered }: { papers: ShelfPaper[]; discovered: Map<string, PaperMeta> }) {
  return (
    <div className="paper-shelf mt-0.5 mb-2 ms-7 grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-1.5">
      {papers.map((paper) => (
        <PaperCard key={`${paper.source}:${paper.id}`} paper={paper} discovered={discovered.get(paper.id)} />
      ))}
    </div>
  );
}

function Sparkline({ metric }: { metric: TrackioMetric }) {
  const width = 120;
  const height = 24;
  const steps = metric.series.map((point) => point.step);
  const values = metric.series.map((point) => point.value);
  const [x0, x1] = [Math.min(...steps), Math.max(...steps)];
  const [y0, y1] = [Math.min(...values), Math.max(...values)];
  const x = (step: number) => 1 + ((step - x0) / (x1 - x0 || 1)) * (width - 2);
  const y = (value: number) => height - 2 - ((value - y0) / (y1 - y0 || 1)) * (height - 4);
  const path = metric.series.map((point, index) => `${index ? "L" : "M"}${x(point.step).toFixed(1)} ${y(point.value).toFixed(1)}`).join(" ");
  const last = metric.series.at(-1)!;
  return (
    <svg className="shrink-0 text-accent-teal" width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      <path d={path} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinejoin="round" />
      <circle cx={x(last.step)} cy={y(last.value)} r={2.2} fill="currentColor" />
    </svg>
  );
}

/** The papers an answer links, below it: the pages it cites, or that this
 * turn never read the paper. */
export function SourceStrip({ papers, discovered }: { papers: CitedPaper[]; discovered: Map<string, PaperMeta> }) {
  const pages = (paper: CitedPaper) => paper.pages.length > 0
    ? m.chat_sources_pages({ pages: [...paper.pages].sort((a, b) => a - b).map((page) => fmtNumber(page)).join(", ") })
    : undefined;
  return (
    <section className="source-strip mt-3" aria-label={m.chat_sources()}>
      <div className="mb-1.5 text-xs text-muted">{m.chat_sources()}</div>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-1.5">
        {papers.map((paper) => (
          <PaperCard
            key={`${paper.source}:${paper.id}`}
            paper={paper}
            discovered={discovered.get(paper.id)}
            note={paper.read ? pages(paper) : m.chat_sources_not_read()}
            oneLine
          />
        ))}
      </div>
    </section>
  );
}

/** A run the turn launched, waited on or read, with its status and, once it
 * logs to Trackio, its headline metric. */
export function RunCard({ runId, label, pending, partIds, onOpenRun }: {
  runId: string;
  label: string;
  pending: boolean;
  partIds: string[];
  onOpenRun?: (runId: string, intent: TabOpenIntent) => void;
}) {
  const scope = useContext(FileScopeContext);
  const projectId = scope?.projectId ?? "";
  const { listRunsQuery, useTrackioRuns, getTrackioRunMetricsQuery } = useWorkspace().queries;
  const { data: run } = useQuery({
    ...listRunsQuery(projectId),
    enabled: Boolean(projectId),
    select: (runs) => runs.find((candidate) => candidate.id === runId || candidate.id.startsWith(runId)),
  });
  const live = run ? runActive(run.status) : false;
  const trackio = useTrackioRuns(projectId, live).data;
  const logged = Boolean(run && trackio?.runs.includes(run.id));
  const metricsQuery = useQuery({ ...getTrackioRunMetricsQuery(projectId, run?.id ?? runId, live), enabled: logged });
  const metrics = metricsQuery.data;
  const metric = metrics ? headlineMetric(metrics.metrics) : null;
  const metricsError = metricsQuery.error?.message ?? metrics?.errors[0];
  const took = run?.endedAt && run.startedAt ? fmtDuration(run.endedAt - run.startedAt) : null;
  const display = run ? runDisplay(run) : null;
  const value = (number: number) => new Intl.NumberFormat(getLocale(), { maximumSignificantDigits: 4 }).format(number);

  return (
    <div className="run-card relative my-2 rounded-lg border border-border-variant bg-surface py-2 ps-2 pe-3">
      <PartAnchors ids={partIds} />
      <div className="flex items-start gap-2 leading-6">
        <FlaskConical size={16} strokeWidth={1.75} aria-hidden="true" className={`mt-1 shrink-0 ${run?.status === "failed" ? "text-accent-red" : "text-accent-green"}`} />
        <button
          type="button"
          className={`min-w-0 flex-1 text-start text-base text-subtext enabled:cursor-pointer enabled:hover:text-text ${pending ? "tool-running-shimmer" : ""}`}
          disabled={!onOpenRun}
          title={m.a11y_open_logs_for_run({ run: ltr(runId) })}
          {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun?.(runId, intent))}
        >
          {label}
        </button>
        {run && display && (
          <span className="flex h-6 shrink-0 items-center text-sm text-subtext">
            {took && (run.status === "done" || run.status === "failed")
              ? run.status === "done" ? m.run_done_in({ duration: took }) : m.run_failed_after({ duration: took })
              : <StatusBadge {...display} />}
          </span>
        )}
      </div>
      {metric && (
        <div className="ms-6 mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-sm leading-5 text-muted">
          <span className="font-mono text-xs" dir="ltr">{metric.name}</span>
          <span className="text-text tabular-nums">{value(metric.last.value)}</span>
          <Sparkline metric={metric} />
          <span>{live ? m.run_metric_at_step({ step: value(metric.last.step) }) : m.run_metric_from({ value: value(metric.first.value) })}</span>
        </div>
      )}
      {metricsError && (
        <div className="ms-6 mt-1 flex flex-wrap items-center gap-x-2 text-sm leading-5 text-accent-red">
          <span className="min-w-0 wrap-anywhere">{m.run_metrics_unreadable({ error: autoDir(metricsError) })}</span>
          <button
            type="button"
            className="shrink-0 cursor-pointer text-subtext underline hover:text-text focus-visible:outline-2 focus-visible:outline-focus"
            onClick={() => void metricsQuery.refetch()}
          >
            {m.app_retry()}
          </button>
        </div>
      )}
    </div>
  );
}

/** A report the turn wrote to the artifacts directory. */
export function ReportCard({ path, title, lines, partIds, onOpen }: {
  path: string;
  title: string;
  lines: number | null;
  partIds: string[];
  onOpen?: (path: string, intent: TabOpenIntent) => void;
}) {
  const file = path.slice(path.replace(/\\/g, "/").lastIndexOf("/") + 1);
  const lineCount = lines ? pluralMessage(lines, m.report_lines_one, m.report_lines_other)({ count: fmtNumber(lines) }) : null;
  const meta = title === file ? lineCount : [ltr(file), lineCount].filter(Boolean).join(" · ");
  return (
    <button
      type="button"
      className="report-card relative my-2 flex w-fit max-w-full items-center gap-2.5 rounded-lg border border-border-variant bg-surface py-2 ps-2.5 pe-3 text-start enabled:cursor-pointer enabled:hover:border-border focus-visible:outline-2 focus-visible:outline-focus"
      disabled={!onOpen}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpen?.(path, intent))}
    >
      <PartAnchors ids={partIds} />
      <span className="flex h-9 w-7.5 shrink-0 items-center justify-center rounded-sm bg-panel text-subtext">
        <NotebookText size={16} strokeWidth={1.75} aria-hidden="true" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-sm leading-5 text-text" dir="auto">{autoDir(title)}</span>
        {meta && <span className="block truncate text-xs leading-4 text-muted">{meta}</span>}
      </span>
    </button>
  );
}

/** Figures the turn made, closed by default because the answer usually shows them. */
export function FigureReveal({ figures, src, onOpen, expansionState, expansionKey }: {
  figures: string[];
  src: (path: string) => string;
  onOpen?: (path: string, intent: TabOpenIntent) => void;
  expansionState?: Map<string, boolean>;
  expansionKey: string;
}) {
  const [open, setOpen] = useState(() => expansionState?.get(expansionKey) ?? false);
  const regionId = useId();
  const toggle = () => {
    expansionState?.set(expansionKey, !open);
    setOpen(!open);
  };
  return (
    <div className="figure-reveal">
      <button
        type="button"
        className="tool-group-summary flex w-fit max-w-full cursor-pointer items-start gap-2 px-1 py-1 text-start text-base leading-6 text-subtext hover:text-text"
        aria-expanded={open}
        aria-controls={regionId}
        onClick={toggle}
      >
        <span className="flex h-6 shrink-0 items-center text-muted"><ChartSpline size={16} strokeWidth={1.75} aria-hidden="true" /></span>
        <span className="min-w-0">{pluralMessage(figures.length, m.activity_plotted_figures_one, m.activity_plotted_figures_other)({ count: figures.length })}</span>
        <ChevronRight size={16} aria-hidden="true" className={`mt-1 shrink-0 text-muted transition-transform duration-120 ease-standard motion-reduce:transition-none ${open ? "rotate-90" : ""}`} />
      </button>
      <div id={regionId} className={`tool-group-disclosure ${open ? "open" : ""}`} aria-hidden={!open} inert={!open}>
        <div className="tool-group-disclosure-inner">
          <div className="mt-1 mb-2 ms-7 grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-1.5">
            {open && figures.map((path) => (
              <button
                key={path}
                type="button"
                className="overflow-hidden rounded-md border border-border-variant bg-white enabled:cursor-pointer focus-visible:outline-2 focus-visible:outline-focus"
                title={path}
                disabled={!onOpen}
                {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpen?.(path, intent))}
              >
                <img className="block aspect-[4/3] w-full object-contain" src={src(path)} alt={path.slice(path.lastIndexOf("/") + 1)} loading="lazy" />
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
