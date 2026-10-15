import type { Experiment, Run } from "./api";

export type ExperimentFilter = "all" | "active" | "failed";
export type ExperimentSort = "recent" | "name";

export function experimentRows(experiments: Experiment[], runs: Run[]) {
  const grouped = new Map<string, Run[]>();
  for (const run of runs) {
    const group = grouped.get(run.experimentId) ?? [];
    group.push(run);
    grouped.set(run.experimentId, group);
  }
  for (const group of grouped.values()) group.sort((a, b) => b.createdAt - a.createdAt);
  return experiments.map((experiment) => {
    const experimentRuns = grouped.get(experiment.id) ?? [];
    const latestRun = experimentRuns[0];
    const liveRun = experimentRuns.find((run) => run.status === "running" || run.status === "starting");
    return {
      experiment,
      runs: experimentRuns,
      latestRun,
      liveRun,
      // An old failure is no longer actionable after a successful retry.
      failed: !liveRun && latestRun?.status === "failed",
      activity: latestRun?.createdAt ?? experiment.createdAt,
    };
  });
}

export function filterExperimentRows(
  rows: ReturnType<typeof experimentRows>,
  { search, filter, sort, locale }: { search: string; filter: ExperimentFilter; sort: ExperimentSort; locale: string },
) {
  const terms = search.trim().toLocaleLowerCase(locale).split(/\s+/).filter(Boolean);
  return rows.filter((row) => {
    if (filter === "active" && !row.liveRun) return false;
    if (filter === "failed" && !row.failed) return false;
    const experiment = row.experiment;
    const text = [experiment.title, experiment.slug, experiment.branchName].join(" ").toLocaleLowerCase(locale);
    return terms.every((term) => text.includes(term));
  }).sort((a, b) => {
    if (sort === "name") {
      const byName = (a.experiment.title || a.experiment.slug).localeCompare(b.experiment.title || b.experiment.slug, locale, { numeric: true, sensitivity: "base" });
      if (byName) return byName;
    }
    return b.activity - a.activity || a.experiment.id.localeCompare(b.experiment.id);
  });
}
