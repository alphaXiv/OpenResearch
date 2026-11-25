import type { AlphaXivFolder, ChatPart } from "../api";
import { isParentUpdate } from "../chatRendering";
import type { ProjectProposal } from "../components/NoProject";
import { autoDir, fmtNumber, ltr } from "../i18n";
import type { OrxLitCall } from "../orxCommand";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import type { Glyph } from "./glyphs";
import { normalizedTargetIds, orxShellActivity, spawnedSessionIds, TOOL_TARGET_INSPECTION_LIMIT, TOOL_TARGET_LIMIT } from "./orx";
import { extractHeredocs, meaningfulCommand, shellBody, withVariables } from "./shell";
import {
  baseName,
  fileGlyph,
  harnessOutputFile,
  listNames,
  patchFiles,
  shellSteps,
  summarizeSteps,
  type ActivityFile,
  type ToolActivityKind,
} from "./steps";

export type { ActivityFile, ToolActivityKind };

export interface ToolActivity {
  kind: ToolActivityKind;
  label: string;
  progressLabel?: string;
  /** The row's glyph, when the kind's own says too little. */
  icon?: Glyph;
  /** What an approval card asks to do, when the kind's verb alone says too little. */
  approvalLabel?: string;
  /** Files the label names, each a link where its name appears. */
  files?: ActivityFile[];
  /** What else a compound shell call did, after the step its label names. */
  more?: string[];
  /** The command a shell call ran, for the row's tooltip. */
  command?: string;
  litCall?: OrxLitCall;
  /** Every paper an `orx paper` call reads, when it reads more than one or reads them in a loop. */
  paperIds?: string[];
  queries?: string[];
  runIds?: string[];
  experimentIds?: string[];
  /** Chats the call started, such as with `orx agent spawn` or `delegate`. */
  spawnedSessionIds?: string[];
  /** The label naming the experiment of the call's one run, in place of `label` and `progressLabel`. */
  runLabel?: (experiment: string) => string;
  runProgressLabel?: (experiment: string) => string;
  /** A row about the chat itself, such as a compaction, which stays out of the research summary. */
  aside?: boolean;
  /** Reads one of orx's research guides. */
  guide?: boolean;
  /** The project an `orx project create` call proposed, once the command accepted it. */
  projectProposal?: ProjectProposal;
}

export function inputString(input: Record<string, unknown>, ...keys: string[]): string | null {
  for (const key of keys) {
    const value = input[key];
    if (typeof value === "string" && value) return value;
  }
  return null;
}

function arrayInputString(input: Record<string, unknown>, key: string, field: string): string | null {
  const values = input[key];
  if (!Array.isArray(values)) return null;
  for (const value of values) {
    if (!value || typeof value !== "object" || !(field in value)) continue;
    const candidate = value[field];
    if (typeof candidate === "string" && candidate) return candidate;
  }
  return null;
}

function inputStringArray(input: Record<string, unknown>, key: string): string[] {
  const values = input[key];
  if (!Array.isArray(values)) return [];
  const strings: string[] = [];
  for (let index = 0; index < Math.min(values.length, TOOL_TARGET_INSPECTION_LIMIT); index++) {
    if (typeof values[index] === "string") strings.push(values[index]);
    if (strings.length >= TOOL_TARGET_LIMIT) break;
  }
  return strings;
}

export function exactInputStringArray(input: Record<string, unknown>, key: string): string[] | null {
  const values = input[key];
  if (!Array.isArray(values)) return null;
  const strings: string[] = [];
  for (const value of values) {
    if (typeof value !== "string") return null;
    strings.push(value);
  }
  return strings;
}

function skillNameFromPath(path: string): string | null {
  const parts = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.at(-1)?.toLowerCase() !== "skill.md") return null;
  return parts.at(-2) ?? null;
}

function nativeOrxSkillPath(tool: string, skillName: string): string | null {
  if (!/^orx-[a-z0-9]+(?:-[a-z0-9]+)*$/.test(skillName)) return null;
  if (tool === "Skill") return `.claude/skills/${skillName}/SKILL.md`;
  if (tool === "skill") return `.opencode/skills/${skillName}/SKILL.md`;
  return null;
}

/** What loading each of orx's research guides prepares the agent to do. */
const GUIDE_LABELS: Record<string, [done: () => string, progress: () => string]> = {
  "orx-agent-delegation": [m.activity_guide_delegation, m.activity_guide_delegation_progress],
  "orx-compute": [m.activity_guide_compute, m.activity_guide_compute_progress],
  "orx-create": [m.activity_guide_create, m.activity_guide_create_progress],
  "orx-customize": [m.activity_guide_customize, m.activity_guide_customize_progress],
  "orx-experiment-tree": [m.activity_guide_experiment_tree, m.activity_guide_experiment_tree_progress],
  "orx-feedback": [m.activity_guide_feedback, m.activity_guide_feedback_progress],
  "orx-figures": [m.activity_guide_figures, m.activity_guide_figures_progress],
  "orx-git": [m.activity_guide_git, m.activity_guide_git_progress],
  "orx-instances": [m.activity_guide_instances, m.activity_guide_instances_progress],
  "orx-lit-review": [m.activity_guide_lit_review, m.activity_guide_lit_review_progress],
  "orx-logs": [m.activity_guide_logs, m.activity_guide_logs_progress],
  "orx-paper": [m.activity_guide_paper, m.activity_guide_paper_progress],
  "orx-project-admin": [m.activity_guide_project, m.activity_guide_project_progress],
  "orx-python-env": [m.activity_guide_python, m.activity_guide_python_progress],
  "orx-reports": [m.activity_guide_reports, m.activity_guide_reports_progress],
  "orx-results": [m.activity_guide_results, m.activity_guide_results_progress],
};

/** A skill read or loaded: orx's own are research guides, each named for what it prepares the agent to do. */
function skillActivity(skillName: string, label: string, progressLabel: string, path?: string | null): ToolActivity {
  const activity: ToolActivity = {
    kind: "skill",
    icon: "book",
    label,
    progressLabel,
    files: path ? [{ name: skillName, path }] : undefined,
  };
  if (!skillName.startsWith("orx-")) return activity;
  const labels = GUIDE_LABELS[skillName];
  return labels
    ? { ...activity, guide: true, label: labels[0](), progressLabel: labels[1]() }
    : { ...activity, guide: true, progressLabel: m.activity_loading_research_guide() };
}

/** A file read by a file tool: an image viewed, a skill or guide loaded, a harness's saved output, or a file. */
function readActivity(path: string | null): ToolActivity {
  if (!path) return { kind: "read", icon: "file-text", label: m.chat_panel_read_a_file(), progressLabel: m.activity_reading_a_file() };
  const skillName = skillNameFromPath(path);
  if (skillName) return skillActivity(skillName, m.activity_read_skill({ name: ltr(skillName) }), m.activity_reading_skill({ name: ltr(skillName) }), path);
  if (harnessOutputFile(path)) {
    return { kind: "read", icon: "scroll", label: m.activity_read_background_output(), progressLabel: m.activity_reading_background_output() };
  }
  const name = baseName(path);
  const glyph = fileGlyph(name);
  const target = ltr(name);
  return glyph === "image"
    ? { kind: "read", icon: glyph, label: m.activity_viewed_target({ target }), progressLabel: m.activity_viewing_target({ target }), files: [{ name, path }] }
    : { kind: "read", icon: glyph, label: m.activity_read_target({ target }), progressLabel: m.activity_reading_target({ target }), files: [{ name, path }] };
}

type EditVerb = "edited" | "created" | "deleted" | "wrote";

/** Files a call changed, each with what happened to it. */
function editActivity(changes: Array<{ path: string; verb: EditVerb }>): ToolActivity {
  if (changes.length === 0) return { kind: "edit", icon: "file-pen", label: m.chat_panel_edited_a_file(), progressLabel: m.activity_editing_a_file() };
  const verbs = new Set(changes.map((change) => change.verb));
  const verb: EditVerb = verbs.size === 1 ? changes[0].verb : "edited";
  const files = changes.map(({ path }) => ({ name: baseName(path), path }));
  const target = listNames(files.map((file) => ltr(file.name)));
  const phrasing: Record<EditVerb, [string, string, Glyph]> = {
    edited: [m.activity_edited_target({ target }), m.activity_editing_target({ target }), "file-pen"],
    created: [m.activity_created_target({ target }), m.activity_creating_target({ target }), "file-plus"],
    deleted: [m.activity_deleted_target({ target }), m.activity_deleting_target({ target }), "file-x"],
    wrote: [m.activity_wrote_target({ target }), m.activity_writing_target({ target }), "file-pen"],
  };
  const [label, progressLabel, icon] = phrasing[verb];
  // A deleted file has nothing left to open.
  return { kind: "edit", icon, label, progressLabel, files: verb === "deleted" ? undefined : files };
}

function codexChanges(input: Record<string, unknown>): Array<{ path: string; verb: EditVerb }> {
  const changes = input.changes;
  if (!Array.isArray(changes)) return [];
  const result: Array<{ path: string; verb: EditVerb }> = [];
  for (const change of changes) {
    if (!change || typeof change !== "object" || !("path" in change) || typeof change.path !== "string") continue;
    const kind = "kind" in change ? change.kind : null;
    const type = kind && typeof kind === "object" && "type" in kind ? kind.type : null;
    result.push({ path: change.path, verb: type === "add" ? "created" : type === "delete" ? "deleted" : "edited" });
  }
  return result;
}

/** User-facing activity inferred from a tool call's structured input. */
const toolActivities = new WeakMap<ChatPart, { locale: string; activity: ToolActivity }>();

export function toolActivity(part: ChatPart): ToolActivity {
  const locale = getLocale();
  const cached = toolActivities.get(part);
  if (cached?.locale === locale) return cached.activity;

  let activity: ToolActivity;
  try {
    activity = computeToolActivity(part);
  } catch (error) {
    console.error("work log could not describe a tool call", part.tool, error);
    activity = plainActivity(part);
  }

  // Stream updates replace parts; weak keys release labels with their transcript.
  toolActivities.set(part, { locale, activity });
  return activity;
}

function computeToolActivity(part: ChatPart): ToolActivity {
  const tool = part.tool ?? "tool";
  const input = part.state?.input ?? {};
  const argumentsValue = input.arguments;
  const argumentsInput = argumentsValue && typeof argumentsValue === "object" && !Array.isArray(argumentsValue)
    ? Object.fromEntries(Object.entries(argumentsValue))
    : {};
  const normalizedInput: Record<string, unknown> = { ...input, ...argumentsInput };
  const filePath = inputString(normalizedInput, "filePath", "file_path", "notebookPath", "notebook_path", "path");
  const description = inputString(normalizedInput, "description");
  // A sub-agent's row is its identity, the same while it runs; liveness is the shimmer.
  const subagent = (label: string): ToolActivity => ({ kind: "agent", icon: "users", label, progressLabel: label });
  if (part.state?.title === "subagent") return subagent(description ?? m.activity_ran_subagent());
  const toolNames = [tool, part.state?.title];
  const alphaxivTool = toolNames.map((name) => name?.match(ALPHAXIV_TOOL)?.[1]).find((name) => name != null);
  if (alphaxivTool) return alphaxivActivity(alphaxivTool.toLowerCase(), normalizedInput);
  const orxTool = toolNames.map((name) => name?.match(ORX_TOOL)?.[1]?.toLowerCase()).find((name) => name != null && ORX_TOOLS.has(name));
  if (orxTool) return orxToolActivity(orxTool, normalizedInput, part.state?.output);
  const toolSegments = tool.toLowerCase().split(/(?::|\.|__)+/);
  const baseTool = toolSegments.at(-1) ?? tool.toLowerCase();
  if (baseTool === "run" && toolSegments.includes("web")) return codexWebActivity(normalizedInput);
  const normalizedTool = TOOL_ALIASES.get(baseTool) ?? baseTool;
  switch (normalizedTool) {
    case "shell":
    case "bash":
      return shellActivity(part, normalizedInput);
    case "skill": {
      const skillName = inputString(normalizedInput, "skill", "name");
      if (!skillName) return { kind: "skill", icon: "book", label: m.activity_loaded_a_skill(), progressLabel: m.activity_loading_a_skill() };
      return skillActivity(skillName, m.activity_loaded_skill({ name: ltr(skillName) }), m.activity_loading_skill({ name: ltr(skillName) }), nativeOrxSkillPath(tool, skillName));
    }
    case "read":
      return readActivity(filePath);
    case "imageview":
      return filePath ? readActivity(filePath) : { kind: "read", icon: "image", label: m.activity_viewed_an_image(), progressLabel: m.activity_viewing_an_image() };
    case "write":
      return editActivity(filePath ? [{ path: filePath, verb: "wrote" }] : []);
    case "edit":
    case "multiedit":
    case "notebookedit": {
      const changes = codexChanges(normalizedInput);
      return editActivity(changes.length > 0 ? changes : filePath ? [{ path: filePath, verb: "edited" }] : []);
    }
    case "patch":
    case "apply_patch": {
      const patch = inputString(normalizedInput, "patchText", "patch", "input");
      return editActivity((patch ? patchFiles(patch) : []).map((path) => ({ path, verb: "edited" as const })));
    }
    case "delete":
      return editActivity(filePath ? [{ path: filePath, verb: "deleted" }] : []);
    case "move": {
      const target = filePath ?? inputString(normalizedInput, "source", "from");
      return target
        ? { kind: "edit", icon: "move", label: m.activity_moved_target({ target: ltr(baseName(target)) }), progressLabel: m.activity_moving_target({ target: ltr(baseName(target)) }) }
        : { kind: "edit", icon: "move", label: part.state?.title ?? m.chat_panel_edited_a_file() };
    }
    case "grep":
    case "codesearch":
    case "search": {
      const pattern = inputString(normalizedInput, "pattern", "query");
      const path = inputString(normalizedInput, "path");
      const file = path && /\.[A-Za-z0-9]+$/.test(baseName(path)) ? path : null;
      if (!pattern) return { kind: "search", icon: "search", label: part.state?.title ?? m.activity_searched_code(), progressLabel: m.activity_searching_code() };
      const quoted = ltr(pattern);
      if (file) {
        const target = ltr(baseName(file));
        return {
          kind: "search",
          icon: "search",
          label: m.activity_searched_target_for({ target, pattern: quoted }),
          progressLabel: m.activity_searching_target_for({ target, pattern: quoted }),
          files: [{ name: baseName(file), path: file }],
        };
      }
      return { kind: "search", icon: "search", label: m.activity_searched_code_for({ pattern: quoted }), progressLabel: m.activity_searching_code_for({ pattern: quoted }) };
    }
    case "glob": {
      const pattern = inputString(normalizedInput, "pattern");
      return pattern
        ? { kind: "search", icon: "folder-search", label: m.activity_listed_files_matching({ pattern: ltr(pattern) }), progressLabel: m.activity_listing_files_matching({ pattern: ltr(pattern) }) }
        : { kind: "search", icon: "folder-search", label: m.chat_panel_listed_files(), progressLabel: m.activity_listing_files() };
    }
    case "list":
    case "ls": {
      const target = filePath ? baseName(filePath) : null;
      return target
        ? { kind: "search", icon: "folder-search", label: m.activity_listed_target({ target: ltr(target) }), progressLabel: m.activity_listing_target({ target: ltr(target) }) }
        : { kind: "search", icon: "folder-search", label: m.chat_panel_listed_files(), progressLabel: m.activity_listing_files() };
    }
    case "websearch": {
      const query = inputString(normalizedInput, "query");
      const url = inputString(normalizedInput, "url");
      const pattern = inputString(normalizedInput, "pattern");
      if (query && query !== url) return { kind: "web", icon: "globe", label: m.activity_searched_web({ query }), progressLabel: m.activity_searching_web({ query }) };
      if (pattern && url) return { kind: "web", icon: "globe", label: m.activity_searched_on_page({ pattern }), progressLabel: m.activity_searching_on_page({ pattern }) };
      if (url) return webPage(url);
      return { kind: "web", icon: "globe", label: description ?? m.chat_panel_browsed_the_web(), progressLabel: m.activity_browsing() };
    }
    case "webfetch":
    case "fetch": {
      const url = inputString(normalizedInput, "url");
      return url ? webPage(url) : { kind: "web", icon: "globe", label: description ?? part.state?.title ?? m.activity_read_web_page(), progressLabel: m.activity_reading_web_page() };
    }
    case "task":
      return subagent(description ?? m.activity_ran_subagent());
    case "subagent":
      return subagent(isParentUpdate(part) ? m.activity_sent_update_to_parent() : description ?? subagentLine(normalizedInput));
    case "todowrite":
    case "update_plan":
    case "todo":
      return { kind: "command", icon: "todo", label: m.activity_updated_plan(), progressLabel: m.activity_updating_plan(), aside: true };
    case "todoread":
      return { kind: "command", icon: "todo", label: m.activity_read_plan(), progressLabel: m.activity_reading_plan(), aside: true };
    case "toolsearch": {
      const query = inputString(normalizedInput, "query") ?? "";
      const names = query.startsWith("select:") ? query.slice("select:".length).split(",").map((name) => name.trim()).filter(Boolean) : [];
      const listed = listNames(names.map((name) => ltr(name)));
      return names.length > 0
        ? { kind: "command", icon: "wrench", label: m.activity_loaded_tools({ names: listed }), progressLabel: m.activity_loading_tools({ names: listed }), aside: true }
        : { kind: "command", icon: "wrench", label: m.activity_searched_tools({ query: ltr(query) }), progressLabel: m.activity_searching_tools({ query: ltr(query) }), aside: true };
    }
    case "monitor": {
      const watched = description ?? inputString(normalizedInput, "command") ?? "";
      return { kind: "command", icon: "radar", label: m.activity_started_watch({ description: autoDir(watched) }), progressLabel: m.activity_starting_watch({ description: autoDir(watched) }) };
    }
    case "taskoutput":
    case "bashoutput":
      return { kind: "command", icon: "scroll", label: m.activity_checked_background_command(), progressLabel: m.activity_checking_background_command() };
    case "taskstop":
    case "killshell":
    case "killbash":
      return { kind: "command", icon: "stop", label: m.activity_stopped_background_command(), progressLabel: m.activity_stopping_background_command() };
    case "slashcommand": {
      const command = inputString(normalizedInput, "command");
      return command
        ? { kind: "command", icon: "terminal", label: m.activity_ran_command({ command: ltr(command) }), progressLabel: m.activity_running_command({ command: ltr(command) }) }
        : { kind: "command", icon: "terminal", label: m.chat_panel_ran_a_command(), progressLabel: m.activity_running_a_command() };
    }
    case "imagegeneration":
      return { kind: "edit", icon: "image", label: m.activity_generated_image(), progressLabel: m.activity_generating_image() };
    case "error":
      return { kind: "command", label: m.chat_panel_tool_failed() };
    case "importedchat":
      return { kind: "project", icon: "import", label: m.activity_imported_chat(), aside: true };
    case "contextcompaction":
      return {
        kind: "command",
        icon: "shrink",
        label: part.state?.status === "error"
          ? m.activity_compacting_failed()
          // A row left running (the host died mid-compaction) is not a success.
          : part.state?.status === "running"
            ? m.activity_compacting_context()
            : m.activity_compacted_context(),
        progressLabel: m.activity_compacting_context(),
        aside: true,
      };
    default:
      return otherActivity(tool, part.state?.title, normalizedInput, description ?? filePath);
  }
}

const TOOL_ALIASES = new Map([
  ["read_file", "read"],
  ["view", "read"],
  ["view_image", "imageview"],
  ["write_file", "write"],
  ["edit_file", "edit"],
  ["exec", "bash"],
  ["exec_command", "bash"],
  ["run_command", "bash"],
  ["run_shell_command", "bash"],
  ["agent", "task"],
  ["collabagenttoolcall", "subagent"],
  // orx's own compaction marker reads as the agents' automatic one.
  ["compacted", "contextcompaction"],
  ["imported", "importedchat"],
  ["subagentactivity", "subagent"],
  ["list_directory", "list"],
  ["glob_file_search", "glob"],
  ["web_fetch", "webfetch"],
  ["web_search", "websearch"],
]);

/** A tool orx has no words for: an MCP tool by its server, an agent's own titled call, or the tool's name. */
function otherActivity(tool: string, title: string | undefined, input: Record<string, unknown>, detail: string | null): ToolActivity {
  const mcp = /^mcp__(.+?)__(.+)$/.exec(tool) ?? /^([\w-]+):([\w.-]+)$/.exec(tool);
  if (mcp) {
    const [, server, name] = mcp;
    return {
      kind: "command",
      icon: "plug",
      label: m.activity_used_tool_from({ tool: ltr(name), server: autoDir(server) }),
      progressLabel: m.activity_using_tool_from({ tool: ltr(name), server: autoDir(server) }),
    };
  }
  if (title && title !== tool) return { kind: "command", icon: "wrench", label: title };
  const text = detail ?? inputString(input, "command", "query", "url") ?? "";
  return { kind: "command", icon: "wrench", label: text ? `${tool}: ${text}` : tool };
}

function webPage(url: string): ToolActivity {
  const target = ltr(url.replace(/^https?:\/\//, "").replace(/\/$/, ""));
  return { kind: "web", icon: "globe", label: m.activity_read_target({ target }), progressLabel: m.activity_reading_target({ target }) };
}

function codexWebActivity(input: Record<string, unknown>): ToolActivity {
  const query = arrayInputString(input, "search_query", "q");
  const imageQuery = arrayInputString(input, "image_query", "q");
  const pattern = arrayInputString(input, "find", "pattern");
  const web = (label: string, progressLabel: string): ToolActivity => ({ kind: "web", icon: "globe", label, progressLabel });
  if (query) return web(m.activity_searched_web({ query }), m.activity_searching_web({ query }));
  if (imageQuery) return web(m.activity_searched_images({ query: imageQuery }), m.activity_searching_images({ query: imageQuery }));
  if (pattern) return web(m.activity_searched_web_page({ pattern }), m.activity_searching_on_page({ pattern }));
  if (Array.isArray(input.open)) return web(m.chat_panel_opened_web_pages(), m.activity_browsing());
  if (Array.isArray(input.weather)) return web(m.chat_panel_checked_the_weather(), m.activity_browsing());
  if (Array.isArray(input.finance)) return web(m.chat_panel_checked_market_data(), m.activity_browsing());
  if (Array.isArray(input.sports)) return web(m.chat_panel_checked_sports_data(), m.activity_browsing());
  if (Array.isArray(input.time)) return web(m.chat_panel_checked_local_times(), m.activity_browsing());
  return web(m.chat_panel_browsed_the_web(), m.activity_browsing());
}

/** A call as it reads without the recognizers, for one they trip on: its command, or its tool and what it was given. */
export function plainActivity(part: ChatPart): ToolActivity {
  const input = part.state?.input ?? {};
  const command = shellBody(inputString(input, "command", "cmd") ?? "").replace(/\s+/g, " ").trim();
  if (command) return ranCommand(command);
  const detail = inputString(input, "description", "filePath", "file_path", "path") ?? codexChanges(input).map((change) => change.path).join(", ");
  return otherActivity(part.tool ?? "tool", part.state?.title, input, detail || null);
}

/** A command named as it was typed, for one no recognizer describes. */
function ranCommand(command: string): ToolActivity {
  const shown = ltr(command.length > 160 ? `${command.slice(0, 159)}…` : command);
  return { kind: "command", icon: "terminal", label: m.activity_ran_command({ command: shown }), progressLabel: m.activity_running_command({ command: shown }), command };
}

function shellActivity(part: ChatPart, input: Record<string, unknown>): ToolActivity {
  const rawCommand = inputString(input, "command", "cmd");
  const commandArgv = exactInputStringArray(input, "commandArgv");
  const workdir = inputString(input, "cwd", "workdir");
  const output = part.state?.output || part.state?.error;
  const background = input.run_in_background === true;
  if (!rawCommand && !commandArgv?.length) {
    // ACP agents may name the command only in the call's title.
    const titled = part.state?.title ? withVariables(meaningfulCommand(part.state.title)) : "";
    const orx = titled ? orxShellActivity({ command: titled, commandArgv: null, output, completed: part.state?.status === "completed", runIds: [], experimentIds: [], legacyIds: [] }) : null;
    if (orx?.projectProposal || orx?.kind === "project") return orx;
    return { kind: "command", icon: "terminal", label: m.chat_panel_ran_a_command(), progressLabel: m.activity_running_a_command() };
  }
  const script = shellBody(rawCommand ?? commandArgv?.join(" ") ?? "");
  const { text, bodies } = extractHeredocs(script);
  // orx's recognizers read the script on one line; steps read it as written, so quoted names keep their spaces.
  const command = withVariables(text.replace(/[\t\r ]+/g, " ").trim());
  const inBackground = (activity: ToolActivity): ToolActivity => background
    ? { ...activity, label: m.activity_in_background({ activity: activity.label }), progressLabel: m.activity_in_background({ activity: activity.progressLabel ?? activity.label }) }
    : activity;

  const orx = orxShellActivity({
    command,
    commandArgv,
    output,
    completed: part.state?.status === "completed",
    runIds: normalizedTargetIds(inputStringArray(input, "runTargetIds")),
    experimentIds: normalizedTargetIds(inputStringArray(input, "experimentTargetIds")),
    legacyIds: normalizedTargetIds(inputStringArray(input, "targetIds")),
  });
  if (orx) return inBackground({ ...orx, command });

  const summary = summarizeSteps(shellSteps(withVariables(text.trim()), bodies, workdir));
  if (!summary) return inBackground(ranCommand(command));
  return inBackground({
    kind: summary.kind,
    icon: summary.icon,
    label: summary.label,
    progressLabel: summary.progress,
    files: summary.files.length > 0 ? summary.files : undefined,
    more: summary.more.length > 0 ? summary.more : undefined,
    command,
  });
}

/** orx's own tools for working with other chats, by any harness's name for them. */
const ORX_TOOL = /^(?:tool: )?(?:mcp__)?orx(?:__|[:/_])([a-z_]+)$/i;
const ORX_TOOLS = new Set(["models", "delegate", "subscribe", "unsubscribe", "wait", "status", "read", "chats", "send", "answer"]);

function orxToolActivity(tool: string, input: Record<string, unknown>, output: string | undefined): ToolActivity {
  const chat = (icon: Glyph, label: string, progressLabel: string): ToolActivity => ({ kind: "agent", icon, label, progressLabel });
  switch (tool) {
    case "delegate": {
      const model = inputString(input, "model");
      const title = inputString(input, "title");
      const named = model ? ltr(model) : null;
      const activity = title && named
        ? chat("users", m.activity_delegated_to({ title: autoDir(title), model: named }), m.activity_delegating_to({ title: autoDir(title), model: named }))
        : named
          ? chat("users", m.activity_delegated_task_to({ model: named }), m.activity_delegating_task_to({ model: named }))
          : chat("users", m.chat_panel_delegated_a_task_to_a_new_agent(), m.activity_delegating());
      return { ...activity, spawnedSessionIds: spawnedSessionIds(output) };
    }
    case "send":
      return chat("send", m.activity_messaged_chat(), m.activity_messaging_chat());
    case "status":
      return chat("activity", m.activity_checked_chat(), m.activity_checking_chat());
    case "read":
      return chat("messages", m.activity_read_chat(), m.activity_reading_chat());
    case "chats":
      return chat("messages", m.activity_looked_up_chats(), m.activity_looking_up_chats());
    case "models":
      return chat("boxes", m.activity_listed_models(), m.activity_listing_models());
    case "wait":
      return chat("hourglass", m.activity_waited_for_chats(), m.activity_waiting_for_chats());
    case "subscribe":
      return chat("bell", m.activity_subscribed_to_chats(), m.activity_subscribing_to_chats());
    case "unsubscribe":
      return chat("bell", m.activity_unsubscribed_from_chats(), m.activity_unsubscribing_from_chats());
    default:
      return chat("message", m.activity_answered_question(), m.activity_answering_question());
  }
}

/** The tool of an alphaXiv MCP call: Claude names it `mcp__alphaxiv__x`, Codex
 * `alphaxiv:x` (`orx_alphaxiv:x` when the user configured their own `alphaxiv`),
 * OpenCode `orx_alphaxiv_x`; ACP agents put such a name, or `Tool: alphaxiv/x`,
 * in the title. Anchored so commands and URLs never match. */
export const ALPHAXIV_TOOL = /^(?:tool: )?(?:mcp__)?(?:orx_)?alphaxiv(?:__|[:/_])([a-z0-9_]+)$/i;

function alphaxivActivity(tool: string, input: Record<string, unknown>): ToolActivity {
  const row = (kind: ToolActivityKind, label: string, progressLabel: string, paper?: string, approvalLabel?: string): ToolActivity => ({
    kind,
    label,
    progressLabel,
    approvalLabel,
    // Paper arguments may be full titles, which have no page to link.
    litCall: { kind: "paper", source: "alphaxiv", id: paper && !/\s/.test(paper) ? paper : undefined },
    // Saving a paper or reading notes on it does not read it.
    paperIds: [],
  });
  const read = (label: string, paper: string | null): ToolActivity => ({
    ...row("read", label, paper ? m.activity_reading_target({ target: ltr(paper) }) : m.activity_reading_paper(), paper ?? undefined),
    paperIds: paper && !/\s/.test(paper) ? [paper] : [],
  });
  const readPaper = (paper: string | null) =>
    paper ? m.activity_read_target({ target: ltr(paper) }) : m.activity_read_paper();
  const forQuery = (activity: string, query: string | null) =>
    query ? m.activity_for_query({ activity, query }) : activity;

  switch (tool) {
    case "list_library":
      return row("read", m.activity_alphaxiv_read_library(), m.activity_alphaxiv_reading_library());
    case "list_annotated_papers":
      return row("read", m.activity_alphaxiv_listed_annotated_papers(), m.activity_alphaxiv_reading_library());
    case "get_paper_notes": {
      const paper = inputString(input, "paper_id_or_url");
      const label = paper
        ? m.activity_alphaxiv_read_notes_on({ paper: ltr(paper) })
        : m.activity_alphaxiv_read_notes_on_a_paper();
      return row("read", label, m.activity_alphaxiv_reading_notes(), paper ?? undefined);
    }
    case "save_papers_to_folder": {
      const papers = exactInputStringArray(input, "paper_ids_or_urls") ?? [];
      const only = papers.length === 1 ? papers[0] : undefined;
      const label = only
        ? m.activity_alphaxiv_saved_paper({ paper: ltr(only) })
        : m.activity_alphaxiv_saved_papers({ count: fmtNumber(papers.length) });
      return row("edit", label, m.activity_alphaxiv_saving_papers(), only, alphaxivApprovalLabel(tool, input));
    }
    case "create_folder": {
      const name = inputString(input, "name");
      return row(
        "edit",
        name ? m.activity_alphaxiv_created_folder({ name }) : m.activity_alphaxiv_created_a_folder(),
        m.activity_alphaxiv_creating_folder(),
        undefined,
        alphaxivApprovalLabel(tool, input),
      );
    }
    case "answer_pdf_queries": {
      const paper = inputString(input, "paper");
      const queries = typeof input.queries === "string" ? [input.queries] : exactInputStringArray(input, "queries") ?? [];
      const label = queries.length > 1
        ? m.activity_for_queries({ activity: readPaper(paper), count: fmtNumber(queries.length) })
        : forQuery(readPaper(paper), queries[0] ?? null);
      return read(label, paper);
    }
    case "get_paper_content": {
      const paper = inputString(input, "url");
      return read(readPaper(paper), paper);
    }
    case "read_files_from_github_repository": {
      const repository = inputString(input, "githubUrl")
        ?.replace(/^https?:\/\/(?:www\.)?github\.com\//i, "")
        .replace(/(?:\.git)?\/*$/, "");
      const path = inputString(input, "path")?.replace(/^\/+/, "");
      const target = repository && path ? `${repository}/${path}` : repository;
      return target
        ? row("read", m.activity_read_target({ target: ltr(target) }), m.activity_reading_target({ target: ltr(target) }))
        : row("read", m.activity_alphaxiv_read_github_repository(), m.activity_alphaxiv_reading_github_repository());
    }
    case "discover_papers": {
      const keywords = exactInputStringArray(input, "keywords") ?? [];
      const query = inputString(input, "question") ?? (keywords.join(", ") || null);
      return row("search", forQuery(m.activity_searched_alphaxiv(), query), query ? m.activity_searching_literature_for({ query }) : m.activity_searching_literature());
    }
    case "find_researchers":
      return row("search", forQuery(m.activity_alphaxiv_searched_researchers(), inputString(input, "topic", "query")), m.activity_alphaxiv_searching_researchers());
    case "get_researcher":
    case "resolve_researchers": {
      const count = Array.isArray(input.people)
        ? input.people.length
        : exactInputStringArray(input, "researchers")?.length ?? 0;
      const only = count === 1
        ? arrayInputString(input, "people", "name") ?? exactInputStringArray(input, "researchers")?.[0]
        : null;
      const label = only
        ? m.activity_alphaxiv_looked_up_researcher({ name: only })
        : count > 1
          ? m.activity_alphaxiv_looked_up_researchers({ count: fmtNumber(count) })
          : m.activity_alphaxiv_looked_up_some_researchers();
      return row("search", label, m.activity_alphaxiv_searching_researchers());
    }
    case "get_researcher_papers": {
      const researchers = exactInputStringArray(input, "researchers") ?? [];
      const label = researchers.length === 1
        ? m.activity_alphaxiv_listed_papers_by({ name: researchers[0] })
        : researchers.length > 1
          ? m.activity_alphaxiv_listed_papers_by_researchers({ count: fmtNumber(researchers.length) })
          : m.activity_alphaxiv_listed_researcher_papers();
      return row("search", label, m.activity_alphaxiv_searching_researchers());
    }
    case "get_hot_feed":
      return row("read", m.activity_alphaxiv_checked_trending_papers(), m.activity_alphaxiv_checking_trending_papers());
    default:
      return row("command", m.activity_used_alphaxiv(), m.activity_using_alphaxiv());
  }
}

/** What an alphaXiv write asks to do, naming its folders once `folders` has them. */
export function alphaxivApprovalLabel(
  tool: string,
  input: Record<string, unknown>,
  folders?: AlphaXivFolder[],
): string | undefined {
  const folderName = (key: string) => {
    const id = inputString(input, key);
    return id ? folders?.find((folder) => folder.id === id)?.name : undefined;
  };

  switch (tool) {
    case "save_papers_to_folder": {
      const papers = exactInputStringArray(input, "paper_ids_or_urls") ?? [];
      const folder = folderName("folder_id");
      if (papers.length === 1) {
        return folder ? m.activity_alphaxiv_save_a_paper_to({ folder }) : m.activity_alphaxiv_save_a_paper();
      }
      const count = fmtNumber(papers.length);
      return folder ? m.activity_alphaxiv_save_papers_to({ count, folder }) : m.activity_alphaxiv_save_papers({ count });
    }
    case "create_folder": {
      const name = inputString(input, "name");
      const parent = folderName("parent_folder_id");
      if (!name) return m.activity_alphaxiv_create_a_folder();
      return parent ? m.activity_alphaxiv_create_folder_in({ name, parent }) : m.activity_alphaxiv_create_folder({ name });
    }
  }
}

/** Readable one-liner for a Codex sub-agent spawn/activity row, from the
 * collab item fields the backend put in `state.input`. The model-assigned
 * agent name (`nickname`) is the row's identity when present, matching how
 * Claude rows show the task description, with the generic verb phrasing as
 * the fallback. */
function subagentLine(input: Record<string, unknown>): string {
  const nickname = typeof input.nickname === "string" && input.nickname
    ? input.nickname.replace(/[_-]+/g, " ")
    : "";
  // Sentence-cased for label-initial use; the bare snake_case name stays
  // lowercase when composed mid-sentence ("Sent input to audit experiments").
  const nickLabel = nickname && nickname.charAt(0).toUpperCase() + nickname.slice(1);
  if (nickLabel) return nickLabel;
  switch (typeof input.tool === "string" ? input.tool : "") {
    case "spawnAgent":
      return m.activity_spawned_agent();
    case "sendInput":
      return m.activity_sent_input_to_agent();
    case "resumeAgent":
      return m.activity_resumed_agent();
    case "wait":
      return m.activity_waiting_on_agent();
    case "closeAgent":
      return m.activity_closed_agent();
  }
  switch (typeof input.kind === "string" ? input.kind : "") {
    case "started":
      return m.activity_subagent_started();
    case "interacted":
      // Codex's cross-agent interaction marker: in practice, the agent
      // handing its report up when it finishes.
      return m.activity_agent_reported_back();
    case "interrupted":
      return m.activity_subagent_interrupted();
  }
  return m.activity_subagent();
}

/** The activity as it reads while its call runs. */
export function activityInProgress(activity: ToolActivity): ToolActivity {
  const label = activity.progressLabel ?? {
    skill: m.activity_loading(),
    read: m.activity_reading(),
    search: m.activity_searching(),
    edit: m.activity_editing(),
    project: m.activity_checking_experiments(),
    run: m.activity_launching_experiment(),
    log: m.activity_reading_run_log(),
    web: m.activity_browsing(),
    agent: m.activity_delegating(),
    command: m.activity_running_a_command(),
  }[activity.kind];
  return { ...activity, label, runLabel: activity.runProgressLabel };
}

/** The verb an approval card leads with for a call it asks about. */
export function permissionActivityLabel(tool: string | undefined, input: Record<string, unknown> | undefined): string {
  const activity = toolActivity({
    id: "permission-preview",
    type: "tool",
    tool,
    state: { status: "running", input },
  });
  if (activity.approvalLabel) return activity.approvalLabel;
  return {
    skill: m.activity_load(),
    read: m.activity_read(),
    search: m.activity_search(),
    edit: m.activity_edit(),
    project: m.activity_review(),
    run: m.activity_run(),
    log: m.activity_read(),
    web: m.activity_browse(),
    agent: m.activity_delegate(),
    command: m.activity_run(),
  }[activity.kind];
}
