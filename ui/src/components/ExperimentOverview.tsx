import {
  CalendarDays,
  Clock3,
  FolderTree,
  GitCommitHorizontal,
  Terminal,
} from "lucide-react";
import { useEffect, useState } from "react";
import {
  fmtDuration,
  runDisplayStatus,
  timeAgo,
  type Experiment,
  type Project,
  type Run,
} from "../api";
import { BackendBadge } from "./BackendLogos";
import { BranchPill } from "./BranchPill";
import { Md } from "./Md";
import { StatusBadge } from "./StatusBadge";

const EXPERIMENT_OVERVIEW_ACTION_CLASS_NAME = [
  "experiment-overview-action [display:inline-flex] [align-items:center] [justify-content:center] [gap:7px]",
  "[min-height:36px] [padding:7px_12px] [border:1px_solid_var(--border-variant)] [border-radius:var(--radius-sm)]",
  "[background:var(--base)] [color:var(--text)] [font-size:var(--fs-sm)] [font-weight:var(--fw-semibold)]",
  "[transition:background_120ms_ease,_border-color_120ms_ease]",
  "[&:hover]:[border-color:color-mix(in_oklab,_var(--text)_34%,_var(--border))]",
  "[&:hover]:[background:var(--surface)]",
].join(" ");

const EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME = [
  "experiment-overview-section [margin-top:22px] [padding-top:18px] [border-top:1px_solid_var(--border-variant)]",
  "[&_h2]:[margin:0_0_14px] [&_h2]:[color:var(--text)] [&_h2]:[font-size:var(--fs-md)]",
  "[&_h2]:[font-weight:var(--fw-semibold)]",
].join(" ");

const EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME = [
  "experiment-overview-command [display:block] [margin-top:13px] [color:var(--text)] [font-size:var(--fs-sm)]",
  "[overflow-wrap:anywhere]",
].join(" ");

function fmtDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function runDuration(run: Run, now: number): string {
  return fmtDuration((run.endedAt ?? now) - run.createdAt);
}

export function ExperimentOverview({
  experiment,
  parentExperiment,
  project,
  runs,
  onOpenLogs,
  onOpenCode,
}: {
  experiment: Experiment;
  parentExperiment: Experiment | null;
  project: Project;
  runs: Run[];
  onOpenLogs: (runId: string) => void;
  onOpenCode: () => void;
}) {
  const latestRun = runs[0] ?? null;
  const hasLiveRun = runs.some(
    (run) => run.status === "running" || run.status === "starting",
  );
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!hasLiveRun) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [hasLiveRun]);

  return (
    <div className="experiment-overview [position:absolute] [inset:0] [overflow-y:auto] [background:var(--base)] [&_h1]:[margin:0] [&_h1]:[color:var(--text)] [&_h1]:[font-size:22px] [&_h1]:[line-height:1.25]">
      <div className="experiment-overview-inner [width:100%] [max-width:920px] [margin:0_auto] [padding:26px_28px_40px] [@media((max-width:_720px))]:[padding:20px_18px_32px]">
        <header className="experiment-overview-head [display:flex] [align-items:flex-start] [justify-content:space-between] [gap:24px]">
          <div className="experiment-overview-heading [min-width:0]">
            <h1>{experiment.title || experiment.slug}</h1>
            <div className="experiment-overview-slug [margin-top:5px] [color:var(--muted)] [font-family:var(--mono)] [font-size:var(--fs-sm)]">{experiment.slug}</div>
          </div>
          <StatusBadge status={latestRun ? runDisplayStatus(latestRun) : "idle"} />
        </header>

        <div className="experiment-overview-actions [display:flex] [gap:7px] [margin-top:18px] [@media((max-width:_720px))]:[flex-wrap:wrap]">
          {latestRun && (
            <button
              className={EXPERIMENT_OVERVIEW_ACTION_CLASS_NAME}
              onClick={() => onOpenLogs(latestRun.id)}
            >
              <Terminal size={15} />
              Logs
            </button>
          )}
          <button className={EXPERIMENT_OVERVIEW_ACTION_CLASS_NAME} onClick={onOpenCode}>
            <FolderTree size={15} />
            Code
          </button>
        </div>

        {experiment.description && (
          <section className="experiment-overview-section [margin-top:22px] [padding-top:18px] [border-top:1px_solid_var(--border-variant)] [&_h2]:[margin:0_0_14px] [&_h2]:[color:var(--text)] [&_h2]:[font-size:var(--fs-md)] [&_h2]:[font-weight:var(--fw-semibold)] overview-description [&_.md]:[color:var(--text)] [&_.md]:[line-height:1.65]">
            <h2>Description</h2>
            <Md text={experiment.description} />
          </section>
        )}

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>{latestRun ? "Latest run" : "Runs"}</h2>
          {latestRun && (
            <>
              <div className="experiment-overview-meta [display:flex] [align-items:center] [flex-wrap:wrap] [gap:10px_18px] [color:var(--text)] [font-size:var(--fs-sm)] [&_svg]:[color:var(--muted)] [&_.backend-badge]:[color:var(--text)] [&_.status-badge]:[color:var(--text)] [&_>_span]:[display:inline-flex] [&_>_span]:[align-items:center] [&_>_span]:[gap:5px] [&_code]:[color:var(--text)] [&_code]:[font-size:var(--fs-xs)]">
                <StatusBadge status={runDisplayStatus(latestRun)} />
                <BackendBadge backend={latestRun.backend} />
                <span title="Started">
                  <CalendarDays size={13} />
                  {fmtDate(latestRun.createdAt)}
                </span>
                <span title="Duration">
                  <Clock3 size={13} />
                  {runDuration(latestRun, now)}
                </span>
                {latestRun.commitSha && (
                  <span title="Commit">
                    <GitCommitHorizontal size={14} />
                    <code>{latestRun.commitSha.slice(0, 7)}</code>
                  </span>
                )}
                {latestRun.exitCode !== null &&
                  latestRun.exitCode !== undefined &&
                  latestRun.exitCode !== 0 && (
                    <span>exit {latestRun.exitCode}</span>
                  )}
              </div>
              {latestRun.command && (
                <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {latestRun.command}</code>
              )}
              {latestRun.resultMarkdown && (
                <div
                  className={`experiment-overview-result [margin-top:16px] [&.failed]:[color:var(--accent-red)] ${latestRun.status === "failed" ? "failed" : ""}`}
                >
                  <Md text={latestRun.resultMarkdown} />
                </div>
              )}
            </>
          )}
        </section>

        <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
          <h2>Git</h2>
          <div className="experiment-overview-meta [display:flex] [align-items:center] [flex-wrap:wrap] [color:var(--text)] [font-size:var(--fs-sm)] [&_svg]:[color:var(--muted)] [&_.backend-badge]:[color:var(--text)] [&_.status-badge]:[color:var(--text)] [&_>_span]:[display:inline-flex] [&_>_span]:[align-items:center] [&_>_span]:[gap:5px] [&_code]:[color:var(--text)] [&_code]:[font-size:var(--fs-xs)] experiment-overview-git-meta [gap:9px_14px] [&_.files-pill]:[padding:5px_8px] [&_.files-pill]:[border-radius:var(--radius-sm)] [&_.files-pill_code]:[font-size:var(--fs-xs)]">
            <BranchPill
              owner={project.githubEnabled ? project.githubOwner : ""}
              repo={project.githubEnabled ? project.githubRepo : ""}
              branch={experiment.branchName}
            />
            {parentExperiment && (
              <span>
                from <code>{parentExperiment.slug}</code>
              </span>
            )}
            <span title={fmtDate(experiment.createdAt)}>
              created {timeAgo(experiment.createdAt)}
            </span>
          </div>
          {experiment.runCommand !== latestRun?.command && (
            <code className={EXPERIMENT_OVERVIEW_COMMAND_CLASS_NAME}>$ {experiment.runCommand}</code>
          )}
        </section>

        {runs.length > 0 && (
          <section className={EXPERIMENT_OVERVIEW_SECTION_CLASS_NAME}>
            <h2>Run history</h2>
            <div className="experiment-run-history [border-top:1px_solid_var(--border-variant)] [&_button]:[width:100%] [&_button]:[display:grid] [&_button]:[grid-template-columns:minmax(72px,_0.7fr)_minmax(100px,_1fr)_minmax(70px,_0.7fr)_60px_16px] [&_button]:[align-items:center] [&_button]:[gap:14px] [&_button]:[padding:11px_2px] [&_button]:[border-bottom:1px_solid_var(--border-variant)] [&_button]:[color:var(--text)] [&_button]:[text-align:left] [&_button]:[font-size:var(--fs-sm)] [&_button:hover]:[background:var(--surface)] [@media((max-width:_720px))]:[&_button]:[grid-template-columns:65px_1fr_60px_16px] [@media((max-width:_720px))]:[&_button_>_:nth-child(3)]:[display:none]">
              {runs.map((run, index) => (
                <button key={run.id} onClick={() => onOpenLogs(run.id)}>
                  <span className="experiment-run-number [font-family:var(--mono)] [font-size:var(--fs-xs)] [font-weight:var(--fw-semibold)]">Run {runs.length - index}</span>
                  <StatusBadge status={runDisplayStatus(run)} />
                  <span>{timeAgo(run.createdAt)}</span>
                  <span>{runDuration(run, now)}</span>
                  <Terminal size={13} />
                </button>
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
