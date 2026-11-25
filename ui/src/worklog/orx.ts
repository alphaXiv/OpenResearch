import { autoDir, fmtNumber, ltr } from "../i18n";
import { loopPaperIds, orxArgsMatch, orxArgv, parseOrxLit, recordedDiscoveryQueries, shellWrapperBody } from "../orxCommand";
import { m } from "../paraglide/messages.js";
import { projectProposal, type ProjectProposal } from "../components/NoProject";
import { extractHeredocs, shellSegments, type ShellSegment } from "./shell";
import type { ToolActivity } from "./activity";

export const TOOL_TARGET_LIMIT = 256;
export const TOOL_TARGET_INSPECTION_LIMIT = 1_024;
const TOOL_OUTPUT_SCAN_LIMIT = 20_000;

const UUID_PATTERN = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
/** Chat ids as orx prints them, so a row can link to the chat a call started. The id is only in the output, never the command. */
const SPAWNED_SESSION_PATTERN = new RegExp(`\\bchat_(${UUID_PATTERN})\\b`, "gi");
const RUN_TARGET_PATTERN = `(?:${UUID_PATTERN}|[0-9a-f]{8})`;

export function normalizedTargetIds(...collections: string[][]): string[] {
  const ids = new Set<string>();
  const target = new RegExp(`^${RUN_TARGET_PATTERN}$`, "i");
  let inspected = 0;
  for (const collection of collections) {
    for (const value of collection) {
      if (ids.size >= TOOL_TARGET_LIMIT || inspected++ >= TOOL_TARGET_INSPECTION_LIMIT) return [...ids];
      if (target.test(value)) ids.add(value.toLowerCase());
    }
  }
  return [...ids];
}

function orxCommandSegments(command: string, args: string): ShellSegment[] {
  return shellSegments(command).filter((segment) => orxArgsMatch(segment.raw, args));
}

function commandInvokesOrx(command: string, args: string): boolean {
  return orxCommandSegments(command, args).length > 0;
}

export function spawnedSessionIds(output: string | undefined): string[] {
  if (!output) return [];
  const ids = new Set<string>();
  for (const match of output.slice(0, TOOL_OUTPUT_SCAN_LIMIT).matchAll(SPAWNED_SESSION_PATTERN)) {
    ids.add(match[0].toLowerCase());
    if (ids.size >= TOOL_TARGET_LIMIT) break;
  }
  return [...ids];
}

function idsFromToolOutput(output: string | undefined, resource: "runs" | "experiments"): string[] {
  if (!output) return [];
  const ids = new Set<string>();
  const boundedOutput = output.slice(0, TOOL_OUTPUT_SCAN_LIMIT);
  const patterns = resource === "runs"
    ? [
      new RegExp(`/runs/(${UUID_PATTERN})`, "gi"),
      new RegExp(`\\brun(?:_|\\s+)id:\\s*(${UUID_PATTERN})`, "gi"),
      new RegExp(`^\\s*RUN\\s+(${UUID_PATTERN})\\b`, "gim"),
      new RegExp(`\\bRun\\s+(${UUID_PATTERN})\\b`, "g"),
      new RegExp(`={3,}\\s*(${UUID_PATTERN})\\s*={3,}`, "gi"),
    ]
    : [
      new RegExp(`/experiments/(${UUID_PATTERN})`, "gi"),
      new RegExp(`^\\s*id:\\s*(${UUID_PATTERN})`, "gim"),
      new RegExp(`={3,}\\s*(${UUID_PATTERN})\\s*={3,}`, "gi"),
    ];
  for (const pattern of patterns) {
    for (const match of boundedOutput.matchAll(pattern)) {
      ids.add(match[1]);
      if (ids.size >= TOOL_TARGET_LIMIT) return [...ids];
    }
  }
  const bareIdLine = new RegExp(`^\\s*(${UUID_PATTERN})(?:\\s|$)`, "gim");
  for (const match of boundedOutput.matchAll(bareIdLine)) {
    ids.add(match[1]);
    if (ids.size >= TOOL_TARGET_LIMIT) break;
  }
  return [...ids];
}

function invocationOffsets(command: string, invocations: ShellSegment[]): Array<{ invocation: ShellSegment; offset: number }> {
  let cursor = 0;
  return invocations.map((invocation) => {
    const next = command.indexOf(invocation.raw, cursor);
    const offset = next === -1 ? command.indexOf(invocation.raw) : next;
    cursor = Math.max(cursor, offset + invocation.raw.length);
    return { invocation, offset: Math.max(0, offset) };
  });
}

function assignedIdsBefore(command: string, variable: string, before: number, idPattern: string): string[] {
  const assignment = new RegExp(
    `(?:^|[\\s;])(?:export\\s+)?${variable}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s;]+))`,
    "gi",
  );
  let resolved = "";
  for (const match of command.matchAll(assignment)) {
    if ((match.index ?? 0) >= before) break;
    resolved = match[1] ?? match[2] ?? match[3] ?? "";
  }
  return [...resolved.matchAll(new RegExp(idPattern, "gi"))].map((match) => match[0]);
}

function loopIdsBefore(command: string, variable: string, before: number, idPattern: string): string[] {
  const loop = new RegExp(`\\bfor\\s+${variable}\\s+in\\s+([\\s\\S]*?)(?:;|\\n)\\s*do\\b`, "gi");
  let values = "";
  for (const match of command.matchAll(loop)) {
    const start = match.index ?? 0;
    if (start >= before) break;
    const headerEnd = start + match[0].length;
    if (headerEnd <= before && /\bdone\b/.test(command.slice(headerEnd, before))) continue;
    values = match[1];
  }
  if (/\$\(|`/.test(values)) return [];
  return [...values.matchAll(new RegExp(idPattern, "gi"))].map((match) => match[0]);
}

function commandRunIds(command: string, output?: string, preservedIds: string[] = [], legacyIds: string[] = []): string[] {
  const invocations = orxCommandSegments(command, "logs");
  const ids = new Set<string>();
  if (invocations.length === 0) {
    if (!commandInvokesOrx(command, "logs")) return [];
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "runs");
    for (const id of preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
    return normalizedTargetIds([...ids]);
  }
  let hasUnresolvedTarget = false;
  for (const { invocation, offset } of invocationOffsets(command, invocations)) {
    const argv = orxArgv(invocation.raw);
    if (argv?.[0] !== "logs") continue;
    const words = argv.slice(1);
    let target: string | null = null;
    for (let index = 0; index < words.length; index++) {
      const word = words[index];
      if (word === "--head") continue;
      if (word === "--bytes" || word === "--range") {
        index++;
        continue;
      }
      if (word.startsWith("--bytes=") || word.startsWith("--range=")) continue;
      target = word;
      break;
    }
    if (!target) {
      hasUnresolvedTarget = true;
      continue;
    }
    if (new RegExp(`^${RUN_TARGET_PATTERN}$`, "i").test(target)) {
      ids.add(target);
      continue;
    }
    const variableMatch = /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(target);
    if (!variableMatch) {
      hasUnresolvedTarget = true;
      continue;
    }
    const variable = variableMatch[1];
    const assignmentIds = assignedIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
    for (const id of assignmentIds) ids.add(id);
    const loopIds = loopIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
    for (const id of loopIds) ids.add(id);
    if (assignmentIds.length === 0 && loopIds.length === 0) hasUnresolvedTarget = true;
  }
  if (ids.size === 0 || hasUnresolvedTarget) {
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "runs");
    const fallback = preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds;
    for (const id of fallback) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
  }
  return normalizedTargetIds([...ids]);
}

function commandExperimentIds(command: string, output?: string, preservedIds: string[] = [], legacyIds: string[] = []): string[] {
  const invocations = orxCommandSegments(command, "exp\\s+(?:status|desc)");
  if (invocations.length === 0) return [];
  const ids = new Set<string>();
  let hasUnresolvedTarget = false;
  for (const { invocation, offset } of invocationOffsets(command, invocations)) {
    const argv = orxArgv(invocation.raw);
    const target = argv?.[0] === "exp" && (argv[1] === "status" || argv[1] === "desc")
      ? argv[2]
      : null;
    let resolved = false;
    if (target && new RegExp(`^${RUN_TARGET_PATTERN}$`, "i").test(target)) {
      ids.add(target);
      resolved = true;
    }
    const variableMatch = target ? /^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$/.exec(target) : null;
    if (variableMatch) {
      const variable = variableMatch[1];
      const assignmentIds = assignedIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
      if (assignmentIds.length > 0) {
        for (const id of assignmentIds) ids.add(id);
        resolved = true;
      }
      const loopIds = loopIdsBefore(command, variable, offset, RUN_TARGET_PATTERN);
      for (const id of loopIds) ids.add(id);
      if (loopIds.length > 0) resolved = true;
    }
    if (!resolved) hasUnresolvedTarget = true;
  }
  if (ids.size === 0 || hasUnresolvedTarget) {
    const outputIds = preservedIds.length > 0 ? [] : idsFromToolOutput(output, "experiments");
    const fallback = preservedIds.length > 0 ? preservedIds : outputIds.length > 0 ? outputIds : legacyIds;
    for (const id of fallback) {
      ids.add(id);
      if (ids.size >= TOOL_TARGET_LIMIT) break;
    }
  }
  return normalizedTargetIds([...ids]);
}

interface OrxShellCall {
  command: string;
  commandArgv: string[] | null;
  output: string | undefined;
  completed: boolean;
  runIds: string[];
  experimentIds: string[];
  legacyIds: string[];
}

/** What an `orx` call did to the research: papers, projects, experiments and runs. Null when the script runs no `orx` command orx describes. */
export function orxShellActivity({ command, commandArgv, output, completed, runIds: resourceRunIds, experimentIds: resourceExperimentIds, legacyIds }: OrxShellCall): ToolActivity | null {
  const proposed = (proposal: ProjectProposal): ToolActivity => ({
    kind: "project",
    icon: "flask",
    label: m.activity_proposed_project({ name: autoDir(proposal.name) }),
    progressLabel: m.activity_proposing_project({ name: autoDir(proposal.name) }),
    projectProposal: completed ? proposal : undefined,
  });
  const shellParts = shellSegments(command);
  let literatureInputs: Array<string | readonly string[]> = shellParts.map((segment) => segment.raw);
  if (commandArgv?.length) {
    const wrapperBody = shellWrapperBody(commandArgv);
    literatureInputs = wrapperBody === null
      ? [commandArgv]
      : shellSegments(extractHeredocs(wrapperBody).text.replace(/[\t\r ]+/g, " ").trim()).map((segment) => segment.raw);
  }
  const proposal = literatureInputs.map((input) => projectProposal(orxArgv(input))).find(Boolean);
  if (proposal) return proposed(proposal);
  const litCalls = literatureInputs.flatMap((input) => parseOrxLit(input) ?? []);
  const litCall = litCalls[0] ?? null;
  const hasNonLiteratureOrx = literatureInputs.some((input) => {
    const argv = orxArgv(input);
    return argv !== null && argv[0] !== "discover" && argv[0] !== "paper";
  });
  if (litCall && !hasNonLiteratureOrx) {
    const chainedSearches = litCalls.filter((call) => call.kind === "discover");
    const literatureCommand = commandArgv ? shellWrapperBody(commandArgv) ?? command : command;
    const queries = chainedSearches.length > 1
      ? chainedSearches.flatMap((call) => call.query ? [call.query] : [])
      : litCall.kind === "discover" && !litCall.query
        ? recordedDiscoveryQueries(literatureCommand, output ?? "", litCall.strategy)
        : [];
    const oneStrategy = new Set(chainedSearches.map((call) => call.strategy)).size <= 1;
    const discoveryLabel = litCall.kind === "discover" && oneStrategy
      ? {
        keyword: m.activity_searched_alphaxiv_full_text(),
        embedding: m.activity_searched_alphaxiv_semantically(),
        openalex: m.activity_searched_openalex(),
        biorxiv: m.activity_searched_biorxiv(),
        pubmed: m.activity_searched_pubmed(),
      }[litCall.strategy]
      : null;
    const paperIds = [...new Set([
      ...litCalls.flatMap((call) => call.kind === "paper" && call.id ? [call.id] : []),
      ...loopPaperIds(literatureCommand),
    ])];
    const searched = discoveryLabel ?? m.activity_searched_literature();
    const query = litCall.kind === "discover" ? litCall.query ?? (queries.length === 1 ? queries[0] : null) : null;
    const label = litCall.kind === "paper"
      ? paperIds.length > 1
        ? m.activity_read_papers({ count: fmtNumber(paperIds.length) })
        : paperIds[0] ? m.activity_read_target({ target: ltr(paperIds[0]) }) : m.activity_read_paper()
      : queries.length > 1
        ? m.activity_for_queries({ activity: searched, count: fmtNumber(queries.length) })
        : query ? m.activity_for_query({ activity: searched, query }) : searched;
    const progressLabel = litCall.kind === "paper"
      ? paperIds.length === 1 ? m.activity_reading_target({ target: ltr(paperIds[0]) }) : m.activity_reading_paper()
      : query
        ? m.activity_searching_literature_for({ query })
        : m.activity_searching_literature();
    return { kind: litCall.kind === "paper" ? "read" : "search", label, progressLabel, litCall, queries, paperIds: litCall.kind === "paper" ? paperIds : undefined };
  }

  const lit = litCall ?? undefined;
  if (commandInvokesOrx(command, "agent\\s+spawn")) {
    return {
      kind: "agent",
      icon: "users",
      label: m.chat_panel_delegated_a_task_to_a_new_agent(),
      progressLabel: m.activity_delegating(),
      spawnedSessionIds: spawnedSessionIds(output),
      litCall: lit,
    };
  }
  const readsExperimentStatus = commandInvokesOrx(command, "exp\\s+status");
  const readsExperimentNotes = commandInvokesOrx(command, "exp\\s+desc");
  const updatesExperimentNotes = orxCommandSegments(command, "exp\\s+desc").some((segment) => {
    const argv = orxArgv(segment.raw) ?? [];
    return argv.some((token) => token === "--set" || token.startsWith("--set=") || token === "--stdin");
  });
  if (commandInvokesOrx(command, "logs")) {
    const runIds = commandRunIds(command, output, resourceRunIds, legacyIds);
    return {
      kind: "log",
      icon: "scroll",
      label: runIds.length === 1 ? m.activity_reviewed_run_log() : m.activity_reviewed_run_logs(),
      progressLabel: m.activity_reading_run_log(),
      runIds,
      runLabel: (experiment) => m.activity_read_log_of({ experiment }),
      runProgressLabel: (experiment) => m.activity_reading_log_of({ experiment }),
      litCall: lit,
    };
  }
  const runSegments = orxCommandSegments(command, "exp\\s+run");
  if (runSegments.length > 0 && runSegments.every((segment) => orxArgv(segment.raw)?.includes("--estimate"))) {
    return { kind: "run", icon: "gauge", label: m.activity_estimated_run(), progressLabel: m.activity_estimating_run(), litCall: lit };
  }
  if (runSegments.length > 0) {
    return {
      kind: "run",
      icon: "rocket",
      label: m.chat_panel_started_an_experiment_run(),
      progressLabel: m.activity_launching_experiment(),
      runIds: idsFromToolOutput(output, "runs"),
      runLabel: (experiment) => m.activity_launched_experiment({ experiment }),
      runProgressLabel: (experiment) => m.activity_launching_named_experiment({ experiment }),
      litCall: lit,
    };
  }
  if (commandInvokesOrx(command, "exp\\s+wait")) {
    return {
      kind: "run",
      icon: "hourglass",
      label: m.chat_panel_waited_for_an_experiment_run(),
      progressLabel: m.activity_waiting_on_run(),
      runIds: idsFromToolOutput(output, "runs"),
      runLabel: (experiment) => m.activity_waited_on_experiment({ experiment }),
      runProgressLabel: (experiment) => m.activity_waiting_on_experiment({ experiment }),
      litCall: lit,
    };
  }
  if (commandInvokesOrx(command, "exp\\s+cancel")) {
    return {
      kind: "run",
      icon: "stop",
      label: m.chat_panel_requested_an_experiment_stop(),
      progressLabel: m.activity_stopping_run(),
      runIds: idsFromToolOutput(output, "runs"),
      runLabel: (experiment) => m.activity_stopped_experiment({ experiment }),
      litCall: lit,
    };
  }
  const flags = orxCommandSegments(command, "exp\\s+flag").map((segment) => orxArgv(segment.raw) ?? []);
  if (flags.length > 0) {
    const labels = {
      keep: [m.activity_kept_run_going, (experiment: string) => m.activity_kept_experiment_going({ experiment })],
      stop: [m.chat_panel_requested_an_experiment_stop, (experiment: string) => m.activity_stopped_experiment({ experiment })],
      ask: [m.activity_left_run_choice_to_you, (experiment: string) => m.activity_left_experiment_choice_to_you({ experiment })],
    } as const;
    const settled = flags.map((argv) => ({
      choice: argv.includes("--stop") ? "stop" as const : argv.includes("--ask") ? "ask" as const : "keep" as const,
      runIds: normalizedTargetIds(argv.slice(2).filter((word, index, words) => !word.startsWith("-") && !["--flag", "--reason"].includes(words[index - 1]))),
    }));
    // The row names the first flag's run; flags settled on other runs follow it.
    const [first, ...others] = settled;
    return {
      kind: "run",
      icon: "flag",
      label: labels[first.choice][0](),
      progressLabel: m.activity_settling_flag(),
      runIds: first.runIds,
      runLabel: labels[first.choice][1],
      more: others.length > 0 ? others.map(({ choice }) => labels[choice][0]()) : undefined,
      litCall: lit,
    };
  }
  const views = orxCommandSegments(command, "exp\\s+view");
  if (views.length > 0) {
    const saves = views.some((segment) => orxArgv(segment.raw)?.[2] === "set");
    return saves
      ? { kind: "project", icon: "chart", label: m.activity_saved_run_view(), progressLabel: m.activity_saving_run_view(), litCall: lit }
      : { kind: "project", icon: "chart", label: m.activity_checked_run_view(), progressLabel: m.activity_checking_run_view(), litCall: lit };
  }
  const experimentIds = () => commandExperimentIds(command, output, resourceExperimentIds, legacyIds);
  const project = (label: string, progressLabel: string, ids?: string[]): ToolActivity =>
    ({ kind: "project", icon: "flask", label, progressLabel, experimentIds: ids, litCall: lit });
  const notesLabel = updatesExperimentNotes ? m.activity_updated_experiment_notes() : m.activity_read_experiment_notes();
  const combinedLabel = updatesExperimentNotes ? m.activity_checked_status_updated_notes() : m.activity_reviewed_status_notes();
  const readsProject = commandInvokesOrx(command, "project\\s+view");
  if (readsExperimentStatus && readsExperimentNotes) return project(combinedLabel, m.activity_checking_experiments(), experimentIds());
  if (readsExperimentNotes) return project(notesLabel, updatesExperimentNotes ? m.activity_updating_experiment_notes() : m.activity_checking_experiments(), experimentIds());
  if (readsExperimentStatus) return project(m.chat_panel_checked_experiment_status(), m.activity_checking_experiments(), experimentIds());
  if (readsProject) return project(m.chat_panel_read_project_details(), m.activity_reading_project_details());
  if (commandInvokesOrx(command, "create-experiment")) return project(m.activity_created_an_experiment(), m.activity_creating_an_experiment());
  if (commandInvokesOrx(command, "runs?")) return project(m.chat_panel_listed_project_runs(), m.activity_listing_project_runs());
  if (commandInvokesOrx(command, "projects")) return project(m.chat_panel_listed_projects(), m.activity_listing_projects());
  if (commandInvokesOrx(command, "(?:compute|instances?|orgs)")) {
    return { kind: "project", icon: "cpu", label: m.chat_panel_checked_compute_options(), progressLabel: m.activity_checking_compute_options(), litCall: lit };
  }
  if (commandInvokesOrx(command, "metrics")) {
    return { kind: "log", icon: "chart", label: m.activity_read_run_metrics(), progressLabel: m.activity_reading_run_metrics(), litCall: lit };
  }
  return null;
}
