import { fmtDuration } from "../api";
import { fmtNumber, ltr } from "../i18n";
import { containsShellGlob } from "../orxCommand";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import type { Glyph } from "./glyphs";
import {
  redirectTarget,
  segmentFilePath,
  shellInvocation,
  shellSegments,
  type ShellInvocation,
  type ShellSegment,
} from "./shell";

export type ToolActivityKind = "skill" | "read" | "search" | "edit" | "web" | "agent" | "project" | "run" | "log" | "command";

/** A file a step names, which its label links to. */
export interface ActivityFile {
  name: string;
  path: string;
  /** The revision it was read at, as in `git show <ref>:<path>`. */
  ref?: string;
}

interface Target {
  name: string;
  file?: ActivityFile;
}

/** What a step did to its targets, finished and in progress. */
interface Phrase {
  done: (target: string) => string;
  doing: (target: string) => string;
}

/** One thing a shell call did, such as reading a file or running tests. */
export interface Step {
  kind: ToolActivityKind;
  icon: Glyph;
  /** How much the step says about the call; the highest names it. */
  rank: number;
  /** Steps sharing a phrase merge into one label naming every target. */
  phrase?: Phrase;
  targets?: Target[];
  label?: string;
  progress?: string;
  /** A file the step executes, or writes, so writing a script and running it read as one step. */
  runs?: string;
  writes?: string;
}

/** A shell call as one row: the step that names it, and the others it also did. */
export interface ShellSummary {
  kind: ToolActivityKind;
  icon: Glyph;
  label: string;
  progress: string;
  files: ActivityFile[];
  more: string[];
}

const RANK = {
  edit: 90,
  vcsWrite: 80,
  test: 75,
  build: 70,
  check: 65,
  format: 62,
  install: 60,
  script: 55,
  command: 50,
  remote: 50,
  web: 45,
  github: 45,
  stop: 44,
  files: 42,
  data: 38,
  read: 30,
  vcsRead: 28,
  search: 25,
  list: 20,
  wait: 15,
  info: 10,
} as const;

const NOISE = new Set([
  "cd", "pushd", "popd", "echo", "printf", "true", "false", ":", "set", "export", "unset", "local", "declare", "typeset",
  "source", ".", "trap", "shopt", "ulimit", "umask", "exit", "return", "wait", "clear", "pwd", "date", "test", "[", "[[",
  "for", "done", "fi", "esac", "case", "in", "break", "continue", "read", "let", "eval", "alias", "hash", "sleep", "then",
  "else", "do", "function", "select", "tput", "stty", "say", "osascript", "afplay", "basename", "dirname", "realpath",
  "readlink", "seq", "mktemp", "expr", "yes",
]);

const READERS = new Set(["cat", "bat", "batcat", "less", "more", "nl", "head", "tail", "jq", "yq", "xxd", "hexdump", "od", "strings", "awk", "gawk", "cut", "sort", "uniq", "column", "tac", "file", "stat", "shasum", "sha256sum", "md5", "md5sum", "plutil", "pdftotext", "csvlook"]);
const SEARCHERS = new Set(["rg", "grep", "egrep", "fgrep", "ag", "ack", "git-grep"]);
const IMAGE_FILE = /\.(?:apng|avif|bmp|gif|heic|ico|jpe?g|png|svg|tiff?|webp)$/i;

/** The last path segment, for compact display ("src/a/b.rs" → "b.rs"). */
export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1) || trimmed;
}

/** Up to three names in the locale's list form, the rest counted: "a, b, c, and 2 more". */
export function listNames(names: string[]): string {
  const shown = names.length > 3 ? [...names.slice(0, 3), m.activity_more_names({ count: fmtNumber(names.length - 3) })] : names;
  return new Intl.ListFormat(getLocale(), { type: "conjunction" }).format(shown);
}

/** The word after the first of `flags` in `args`, if any of them is there. */
function optionValue(args: string[], ...flags: string[]): string | undefined {
  const at = args.findIndex((arg) => flags.includes(arg));
  return at < 0 ? undefined : args[at + 1];
}

/** A target a command names on its own line, never a flag, number, expansion or glob. */
function plainTarget(value: string | undefined): string | null {
  const target = value?.replace(/[)'"]+$/, "");
  if (!target || target === "-" || /^\d+$/.test(target) || /[$`]/.test(target) || containsShellGlob(target)) return null;
  return target;
}

/** A target that looks like a file: it has a directory, a leading dot, an extension or a well-known name. */
function fileLike(value: string | undefined): string | null {
  const target = plainTarget(value);
  if (!target) return null;
  const name = baseName(target);
  return target.includes("/") || target.startsWith(".") || /\.[A-Za-z0-9][A-Za-z0-9_-]*$/.test(name)
    || /^(?:Dockerfile|Makefile|README|LICENSE|NOTICE|CHANGELOG|Cargo\.lock)$/i.test(name)
    ? target
    : null;
}

/** Claude Code keeps a background command's output, and a long tool result, in files of its own. */
export function harnessOutputFile(path: string): boolean {
  return /[\\/]tasks[\\/][\w-]+\.output$/.test(path) || /[\\/]tool-results[\\/][^\\/]+$/.test(path);
}

const phrases = {
  read: { done: (target: string) => m.activity_read_target({ target }), doing: (target: string) => m.activity_reading_target({ target }) },
  viewed: { done: (target: string) => m.activity_viewed_target({ target }), doing: (target: string) => m.activity_viewing_target({ target }) },
  counted: { done: (target: string) => m.activity_counted_lines({ target }), doing: (target: string) => m.activity_counting_lines({ target }) },
  edited: { done: (target: string) => m.activity_edited_target({ target }), doing: (target: string) => m.activity_editing_target({ target }) },
  wrote: { done: (target: string) => m.activity_wrote_target({ target }), doing: (target: string) => m.activity_writing_target({ target }) },
  wroteAndRan: { done: (target: string) => m.activity_wrote_and_ran({ target }), doing: (target: string) => m.activity_writing_and_running({ target }) },
  deleted: { done: (target: string) => m.activity_deleted_target({ target }), doing: (target: string) => m.activity_deleting_target({ target }) },
  createdFolder: { done: (target: string) => m.activity_created_folder({ target }), doing: (target: string) => m.activity_creating_folder({ target }) },
  moved: { done: (target: string) => m.activity_moved_target({ target }), doing: (target: string) => m.activity_moving_target({ target }) },
  copied: { done: (target: string) => m.activity_copied_target({ target }), doing: (target: string) => m.activity_copying_target({ target }) },
  listed: { done: (target: string) => m.activity_listed_target({ target }), doing: (target: string) => m.activity_listing_target({ target }) },
  ran: { done: (command: string) => m.activity_ran_command({ command }), doing: (command: string) => m.activity_running_command({ command }) },
  restored: { done: (target: string) => m.activity_restored_target({ target }), doing: (target: string) => m.activity_restoring_target({ target }) },
  installed: { done: (target: string) => m.activity_installed_target({ target }), doing: (target: string) => m.activity_installing_target({ target }) },
  stopped: { done: (target: string) => m.activity_stopped_target({ target }), doing: (target: string) => m.activity_stopping_target({ target }) },
  fetched: { done: (target: string) => m.activity_fetched_target({ target }), doing: (target: string) => m.activity_fetching_target({ target }) },
  queried: { done: (target: string) => m.activity_queried_target({ target }), doing: (target: string) => m.activity_querying_target({ target }) },
  lookedUp: { done: (target: string) => m.activity_looked_up_target({ target }), doing: (target: string) => m.activity_looking_up_target({ target }) },
  help: { done: (target: string) => m.activity_read_help_for({ target }), doing: (target: string) => m.activity_reading_help_for({ target }) },
} satisfies Record<string, Phrase>;

const plain = (kind: ToolActivityKind, icon: Glyph, rank: number, label: string, progress: string): Step =>
  ({ kind, icon, rank, label, progress });

/** Steps a shell script took, in order, with filters and bookkeeping left out. */
export function shellSteps(command: string, bodies: string[], workdir: string | null): Step[] {
  const segments = shellSegments(command, bodies);
  const context: StepContext = { segments, workdir };
  const steps: Step[] = [];
  for (let index = 0; index < segments.length; index++) {
    const step = segmentStep(segments[index], index, context);
    if (step) steps.push(step);
  }
  return steps;
}

interface StepContext {
  segments: ShellSegment[];
  workdir: string | null;
}

function fileTarget(target: string, at: number, context: StepContext, ref?: string): Target {
  const path = segmentFilePath(target, context.segments, at, context.workdir);
  return path ? { name: baseName(target), file: { name: baseName(target), path, ...(ref ? { ref } : {}) } } : { name: baseName(target) };
}

function segmentStep(segment: ShellSegment, at: number, context: StepContext): Step | null {
  const head = segment.raw.split(/\s+/, 1)[0];
  if ((head === "while" || head === "until") && context.segments.slice(at + 1).some((next) => shellInvocation(next.raw)?.name === "sleep")) {
    return plain("command", "hourglass", RANK.wait, m.activity_waited_for_condition(), m.activity_waiting_for_condition());
  }
  const invocation = shellInvocation(segment.raw);
  if (!invocation) return null;
  const { name, args } = invocation;

  const written = redirectTarget(segment);
  if (written && ["cat", "echo", "printf", "tee"].includes(name) && (segment.heredoc !== undefined || name !== "cat" || args.length === 0)) {
    return writeStep(written, at, context);
  }
  if (name === "tee" && !segment.piped && segment.heredoc !== undefined) {
    const target = args.find((arg) => !arg.startsWith("-"));
    if (target) return writeStep(target, at, context);
  }
  if (name === "tee" && segment.piped) {
    const target = args.find((arg) => !arg.startsWith("-") && !arg.startsWith("/dev/"));
    // It saves what the command before it printed: a lesser step than that command.
    return target ? { ...writeStep(target, at, context), rank: RANK.files } : null;
  }
  // A filter shapes the output of the command before it; it is not a step of its own.
  // `xargs` hands the piped text to its command as arguments, so that command is a step.
  if (segment.piped && !/^xargs\b/.test(segment.code.trimStart()) && !["sh", "bash", "zsh", "sqlite3", "python", "python3"].includes(name)) return null;
  if (segment.piped && (name === "python" || name === "python3") && !args.some((arg) => /\.py$/.test(arg))) return null;
  if (NOISE.has(name)) {
    // A wait with nothing else to show is the step.
    if (name === "sleep" && context.segments.every((other) => other === segment || NOISE.has(shellInvocation(other.raw)?.name ?? ":"))) {
      const seconds = durationSeconds(args[0]);
      return seconds === null
        ? plain("command", "hourglass", RANK.wait, m.activity_waited_for_condition(), m.activity_waiting_for_condition())
        : plain("command", "hourglass", RANK.wait, m.activity_waited({ duration: fmtDuration(seconds * 1000) }), m.activity_waiting_for({ duration: fmtDuration(seconds * 1000) }));
    }
    return null;
  }
  if (args.includes("--help") || (name !== "git" && name !== "jj" && args[0] === "help" && args.length <= 2)) {
    return { kind: "command", icon: "help", rank: RANK.info, phrase: phrases.help, targets: [{ name: [name, ...args.filter((arg) => !arg.startsWith("-") && arg !== "help")].slice(0, 3).join(" ") }] };
  }
  if (args.length === 1 && ["--version", "-V", "version"].includes(args[0]) && name !== "orx") {
    return plain("command", "info", RANK.info, m.activity_checked_version({ target: ltr(name) }), m.activity_checking_version({ target: ltr(name) }));
  }

  return readStep(invocation, segment, at, context)
    ?? editStep(invocation, segment, at, context)
    ?? searchStep(invocation, at, context)
    ?? vcsStep(invocation, segment, at, context)
    ?? githubStep(invocation)
    ?? devStep(invocation)
    ?? scriptStep(invocation, segment, at, context)
    ?? systemStep(invocation, at, context)
    // A command inside `$(…)` computes a value; unless it is a step worth naming, it is plumbing.
    ?? (segment.nested ? null : unknownStep(segment));
}

function writeStep(target: string, at: number, context: StepContext): Step {
  const resolved = fileTarget(target, at, context);
  return { kind: "edit", icon: "file-pen", rank: RANK.edit, phrase: phrases.wrote, targets: [resolved], writes: resolved.file?.path };
}

/** A path or URL with each variable in it shown as `*`, the files it may stand for. */
function withoutVariables(value: string): string {
  return value.replace(/\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/g, "*");
}

function unknownStep(segment: ShellSegment): Step {
  const raw = segment.raw
    .replace(/[\t\r ]+/g, " ")
    .replace(/^(?:(?:do|then|else|if|while|until|!|time)\s+)+/, "")
    .replace(/\s+(?:\d|&)?>>?\s*(?:&\d|\/dev\/null)/g, "")
    .trim();
  const command = raw.length > 160 ? `${raw.slice(0, 159)}…` : raw;
  return { kind: "command", icon: "terminal", rank: RANK.command, phrase: phrases.ran, targets: [{ name: command }] };
}

function readStep(invocation: ShellInvocation, segment: ShellSegment, at: number, context: StepContext): Step | null {
  const { name, args } = invocation;
  if (name === "sed" && !args.some((arg) => /^-[a-zA-Z]*i/.test(arg) || arg.startsWith("--in-place"))) {
    return readFiles(sedFiles(args), at, context);
  }
  if (!READERS.has(name)) return null;
  if (name === "cat" && segment.heredoc !== undefined) return null;
  const files: string[] = [];
  const valued = new Set(["-n", "--lines", "-c", "--bytes", "--pid", "-s", "-v", "-F", "-f", "-d", "-k", "-t", "-w", "-o", "--arg", "--argjson", "--slurpfile", "--rawfile"]);
  // `jq` and `awk` take their program first.
  let program = name === "jq" || name === "yq" || name === "awk" || name === "gawk";
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "--") {
      files.push(...args.slice(index + 1));
      break;
    }
    if (name !== "cat" && valued.has(arg)) {
      index += arg === "--arg" || arg === "--argjson" || arg === "--slurpfile" || arg === "--rawfile" ? 2 : 1;
      continue;
    }
    if (arg.startsWith("-") && arg !== "-") continue;
    if (program) {
      program = false;
      continue;
    }
    files.push(arg);
  }
  return readFiles(files, at, context);
}

function sedFiles(args: string[]): string[] {
  let index = 0;
  let scriptGiven = false;
  while (index < args.length && args[index].startsWith("-")) {
    const option = args[index++];
    if (["-e", "--expression", "-f", "--file"].includes(option)) {
      scriptGiven = true;
      index++;
    }
  }
  if (!scriptGiven) index++;
  return args.slice(index).filter((arg) => !arg.startsWith("-"));
}

function readFiles(candidates: string[], at: number, context: StepContext): Step | null {
  const targets = candidates.flatMap((candidate) => looseTarget(candidate, at, context) ?? []);
  if (targets.length === 0) return null;
  if (targets.every((target) => harnessOutputFile(target.file?.path ?? target.name))) {
    return plain("read", "scroll", RANK.read, m.activity_read_background_output(), m.activity_reading_background_output());
  }
  const read = targets.filter((target) => !harnessOutputFile(target.file?.path ?? target.name));
  const images = read.every((target) => IMAGE_FILE.test(target.name));
  return { kind: "read", icon: images ? "image" : fileGlyph(read[0].name), rank: RANK.read, phrase: images ? phrases.viewed : phrases.read, targets: read };
}

/** A path a command names, as a file to open, or, behind a glob or a variable, a name to show. */
function looseTarget(value: string, at: number, context: StepContext): Target | null {
  if (!value || value.startsWith("-") || /^\d+$/.test(value)) return null;
  const plainPath = plainTarget(value);
  if (plainPath) return fileTarget(plainPath, at, context);
  // A loop's `$id.txt` stands for many files, as a glob does.
  const segments = withoutVariables(value).replace(/[\\/]+$/, "").split(/[\\/]/);
  const last = segments.at(-1) ?? value;
  // `skills/{a,b}/SKILL.md` keeps the part that varies.
  const name = /[*?[\]{}$]/.test(last) || segments.length < 2 ? last : segments.slice(-2).join("/");
  return { name };
}

/** The icon of a file by what it holds: prose, data, an image or code. */
export function fileGlyph(name: string): Glyph {
  if (IMAGE_FILE.test(name)) return "image";
  if (/\.(?:md|markdown|mdx|txt|rst|tex|log|out|err|pdf)$/i.test(name)) return "file-text";
  if (/\.(?:json|jsonl|ya?ml|toml|ini|cfg|conf|lock)$/i.test(name)) return "file-braces";
  if (/\.(?:csv|tsv|xlsx?|parquet)$/i.test(name)) return "file-spreadsheet";
  return "file-code";
}

function editStep(invocation: ShellInvocation, segment: ShellSegment, at: number, context: StepContext): Step | null {
  const { name, args } = invocation;
  const edited = (paths: string[]): Step | null => {
    const targets = paths.flatMap((path) => plainTarget(path) ?? []).map((path) => fileTarget(path, at, context));
    return targets.length > 0 ? { kind: "edit", icon: "file-pen", rank: RANK.edit, phrase: phrases.edited, targets } : null;
  };
  if (name === "sed" && args.some((arg) => /^-[a-zA-Z]*i/.test(arg) || arg.startsWith("--in-place"))) {
    // BSD sed takes the backup suffix as the next word, often an empty one.
    const words = args.filter((arg) => arg !== "");
    return edited(sedFiles(words.filter((arg) => !/^-[a-zA-Z]*i/.test(arg) && !arg.startsWith("--in-place"))));
  }
  if (name === "perl" && args.some((arg) => /^-\w*i/.test(arg))) {
    const script = args.findIndex((arg) => /^-\w*e$/.test(arg));
    return edited(script >= 0 ? args.slice(script + 2).filter((arg) => !arg.startsWith("-")) : []);
  }
  if (name === "apply_patch" || (name === "git" && args[0] === "apply")) {
    return edited(patchFiles(segment.heredoc ?? args.find((arg) => arg.includes("*** ")) ?? ""));
  }
  return null;
}

/** The files a patch touches, in apply_patch's or unified diff's headers. */
export function patchFiles(patch: string): string[] {
  const files = new Set<string>();
  for (const match of patch.matchAll(/^(?:\*\*\* (?:Add|Update|Delete) File: |\+\+\+ b\/)(.+)$/gm)) files.add(match[1].trim());
  return [...files];
}

/** The pattern a search looks for, and the one path it searches when it names a single one. */
function searchStep(invocation: ShellInvocation, at: number, context: StepContext): Step | null {
  const { name, args } = invocation;
  if (name === "ls" || name === "tree" || name === "exa" || name === "eza" || (name === "rg" && args.includes("--files")) || name === "fd" || name === "find" || (name === "git" && args[0] === "ls-files") || (name === "jj" && args[0] === "file" && args[1] === "list")) {
    return listStep(invocation);
  }
  const search = SEARCHERS.has(name) ? args : name === "git" && args[0] === "grep" ? args.slice(1) : null;
  if (!search) return null;
  const valued = new Set(["-e", "--regexp", "-f", "--file", "-g", "--glob", "-t", "--type", "-T", "--type-not", "-m", "--max-count", "-A", "-B", "-C", "--context", "--after-context", "--before-context", "--include", "--exclude", "--exclude-dir", "-M", "--max-columns", "--color", "--colors", "-j", "--threads", "--sort", "--max-depth", "-d"]);
  let pattern: string | null = null;
  const paths: string[] = [];
  for (let index = 0; index < search.length; index++) {
    const arg = search[index];
    if ((arg === "-e" || arg === "--regexp") && pattern === null) {
      pattern = search[++index] ?? null;
      continue;
    }
    if (valued.has(arg)) {
      index++;
      continue;
    }
    if (arg === "--") {
      if (pattern === null) pattern = search[++index] ?? null;
      paths.push(...search.slice(index + 1));
      break;
    }
    if (arg.startsWith("-")) continue;
    if (pattern === null) pattern = arg;
    else paths.push(arg);
  }
  if (!pattern) return plain("search", "search", RANK.search, m.activity_searched_code(), m.activity_searching_code());
  const only = paths.length === 1 ? plainTarget(paths[0]) : null;
  const quoted = ltr(pattern);
  if (only && only !== ".") {
    const target = fileTarget(only, at, context);
    return {
      kind: "search",
      icon: "search",
      rank: RANK.search,
      label: m.activity_searched_target_for({ target: ltr(target.name), pattern: quoted }),
      progress: m.activity_searching_target_for({ target: ltr(target.name), pattern: quoted }),
      targets: fileLike(only) && /\.[A-Za-z0-9]+$/.test(target.name) ? [target] : undefined,
    };
  }
  return plain("search", "search", RANK.search, m.activity_searched_code_for({ pattern: quoted }), m.activity_searching_code_for({ pattern: quoted }));
}

function listStep({ name, args }: ShellInvocation): Step {
  const values = new Set(["-g", "--glob", "-t", "--type", "-e", "--extension", "-d", "--max-depth", "-L", "-I", "-maxdepth", "-mindepth", "-type", "-size", "-mtime", "-newer", "-user", "-perm"]);
  const matching = (pattern: string | undefined) => pattern
    ? plain("search", "folder-search", RANK.list, m.activity_listed_files_matching({ pattern: ltr(pattern) }), m.activity_listing_files_matching({ pattern: ltr(pattern) }))
    : null;
  if (name === "find") {
    const root = plainTarget(args.find((arg) => !arg.startsWith("-")));
    return matching(optionValue(args, "-name", "-iname", "-path", "-ipath")) ?? listedDirectory(root);
  }
  if (name === "rg" || name === "fd") {
    const fdPattern = name === "fd" ? args.find((arg) => !arg.startsWith("-")) : undefined;
    return matching(optionValue(args, "-g", "--glob") ?? fdPattern) ?? listedDirectory(null);
  }
  const positional: string[] = [];
  for (let index = 0; index < args.length; index++) {
    if (values.has(args[index])) index++;
    else if (!args[index].startsWith("-")) positional.push(args[index]);
  }
  if (name === "git" || name === "jj") return listedDirectory(null);
  return positional.length === 1 ? listedDirectory(plainTarget(positional[0])) : listedDirectory(null);
}

function listedDirectory(directory: string | null): Step {
  if (!directory || directory === "." || directory === "./") {
    return plain("search", "folder-search", RANK.list, m.chat_panel_listed_files(), m.activity_listing_files());
  }
  return { kind: "search", icon: "folder-search", rank: RANK.list, phrase: phrases.listed, targets: [{ name: baseName(directory) || directory }] };
}

/** A commit or change description's first line, short enough for a row. */
function messageLine(message: string | undefined): string | null {
  const line = message?.split("\n").map((value) => value.trim()).find(Boolean);
  if (!line) return null;
  return line.length > 72 ? `${line.slice(0, 71)}…` : line;
}

function commitMessage(args: string[], segment: ShellSegment, at: number, context: StepContext): string | null {
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (arg === "-m" || arg === "--message") {
      const value = args[index + 1];
      if (value !== undefined && !value.includes("$(")) return messageLine(value);
      // `-m "$(cat <<'EOF' … EOF)"` reads the here-document of the `cat` just before it.
      return messageLine(context.segments.slice(0, at).reverse().find((previous) => previous.heredoc !== undefined)?.heredoc);
    }
    if (arg.startsWith("--message=")) return messageLine(arg.slice("--message=".length));
    if (/^-[a-zA-Z]*m$/.test(arg)) return messageLine(args[index + 1]);
    if ((arg === "-F" || arg === "--file" || arg === "--stdin") && segment.heredoc !== undefined) return messageLine(segment.heredoc);
  }
  return null;
}

function vcsStep(invocation: ShellInvocation, segment: ShellSegment, at: number, context: StepContext): Step | null {
  const { name, args } = invocation;
  if (name !== "git" && name !== "jj") return null;
  const tool = name === "git" ? "Git" : "jj";
  let index = 0;
  // Global options before the subcommand: `git -C dir status`, `jj --no-pager log`.
  while (args[index]?.startsWith("-")) {
    if (["-C", "-c", "-R", "--repository", "--git-dir", "--work-tree", "--at-op", "--config"].includes(args[index])) index++;
    index++;
  }
  const action = args[index];
  const rest = args.slice(index + 1);
  const positional = rest.filter((arg) => !arg.startsWith("-"));
  const valueOf = (...flags: string[]) => optionValue(rest, ...flags);
  const vcs = (icon: Glyph, rank: number, label: string, progress: string) => plain("command", icon, rank, label, progress);
  const status = () => vcs("git-branch", RANK.vcsRead, m.activity_checked_vcs_status({ tool }), m.activity_checking_vcs_status({ tool }));
  const diff = () => vcs("git-compare", RANK.vcsRead, m.chat_panel_reviewed_code_changes(), m.activity_reviewing_code_changes());
  const history = () => vcs("history", RANK.vcsRead, m.activity_read_vcs_history({ tool }), m.activity_reading_vcs_history({ tool }));
  const committed = (message: string | null) => message
    ? vcs("git-commit", RANK.vcsWrite, m.activity_committed({ message: ltr(message) }), m.activity_committing({ message: ltr(message) }))
    : vcs("git-commit", RANK.vcsWrite, m.activity_committed_changes(), m.activity_committing_changes());
  const pushed = (target: string | undefined) => target
    ? vcs("upload", RANK.vcsWrite, m.activity_pushed_target({ target: ltr(target) }), m.activity_pushing_target({ target: ltr(target) }))
    : vcs("upload", RANK.vcsWrite, m.activity_pushed_changes(), m.activity_pushing_changes());
  const pulled = () => vcs("download", RANK.install, m.activity_pulled_changes(), m.activity_pulling_changes());
  const switched = (target: string) => vcs("git-branch", RANK.vcsWrite, m.activity_switched_to({ target: ltr(target) }), m.activity_switching_to({ target: ltr(target) }));
  const rebased = (target: string | undefined) => target
    ? vcs("git-branch", RANK.vcsWrite, m.activity_rebased_onto({ target: ltr(target) }), m.activity_rebasing_onto({ target: ltr(target) }))
    : vcs("git-branch", RANK.vcsWrite, m.activity_rebased_changes(), m.activity_rebasing_changes());
  const restored = (paths: string[]): Step => paths.length > 0
    ? { kind: "edit", icon: "undo", rank: RANK.vcsWrite, phrase: phrases.restored, targets: paths.map((path) => fileTarget(path, at, context)) }
    : vcs("undo", RANK.vcsWrite, m.activity_restored_changes(), m.activity_restoring_changes());
  // A path at a revision is from the repository's root, whatever the working directory.
  const readAt = (ref: string, path: string): Step | null => {
    const file = fileLike(path);
    if (!file) return null;
    const name = baseName(file);
    return { kind: "read", icon: fileGlyph(name), rank: RANK.read, phrase: phrases.read, targets: [{ name, file: { name, path: file, ref } }] };
  };

  if (name === "git") {
    switch (action) {
      case "status":
        return status();
      case "diff":
        return diff();
      case "log":
      case "shortlog":
      case "reflog":
      case "blame":
        return history();
      case "show": {
        const object = positional[0];
        const separator = object?.indexOf(":") ?? -1;
        if (object && separator > 0) return readAt(object.slice(0, separator), object.slice(separator + 1));
        const rev = plainTarget(object) ?? "HEAD";
        return vcs("git-commit", RANK.vcsRead, m.activity_read_commit({ target: ltr(rev) }), m.activity_reading_commit({ target: ltr(rev) }));
      }
      case "commit":
        return committed(commitMessage(rest, segment, at, context));
      case "add":
        return vcs("git-commit", RANK.files, m.activity_staged_changes(), m.activity_staging_changes());
      case "push":
        return pushed(positional[1]);
      case "pull":
      case "fetch":
        return pulled();
      case "checkout":
      case "switch": {
        const created = valueOf("-b", "-B", "-c", "-C", "--orphan");
        if (created) return vcs("git-branch", RANK.vcsWrite, m.activity_created_branch({ target: ltr(created) }), m.activity_creating_branch({ target: ltr(created) }));
        const separator = rest.indexOf("--");
        if (separator >= 0) return restored(rest.slice(separator + 1));
        const target = positional[0];
        return target ? switched(target) : null;
      }
      case "restore":
        return restored(positional);
      case "rebase":
        return rebased(positional[0]);
      case "merge":
        return positional[0] ? vcs("git-merge", RANK.vcsWrite, m.activity_merged_target({ target: ltr(positional[0]) }), m.activity_merging_target({ target: ltr(positional[0]) })) : null;
      case "stash":
        return vcs("git-branch", RANK.vcsWrite, m.activity_stashed_changes(), m.activity_stashing_changes());
      case "branch":
        return positional.length === 0 || rest.includes("--list")
          ? vcs("git-branch", RANK.vcsRead, m.activity_listed_branches(), m.activity_listing_branches())
          : null;
      default:
        return null;
    }
  }

  switch (action) {
    case "st":
    case "status":
      return status();
    case "diff":
    case "interdiff":
      return diff();
    case "log":
    case "obslog":
    case "evolog":
    case "op":
      return history();
    case "show": {
      const rev = plainTarget(positional[0]) ?? "@";
      return vcs("git-commit", RANK.vcsRead, m.activity_read_commit({ target: ltr(rev) }), m.activity_reading_commit({ target: ltr(rev) }));
    }
    case "file": {
      if (rest[0] !== "show") return null;
      const ref = valueOf("-r", "--revision") ?? "@";
      const path = rest.slice(1).filter((arg, at, all) => !arg.startsWith("-") && !["-r", "--revision"].includes(all[at - 1]))[0];
      return path ? readAt(ref, path) : null;
    }
    case "desc":
    case "describe": {
      const message = commitMessage(rest, segment, at, context);
      return message
        ? vcs("git-commit", RANK.vcsWrite, m.activity_described_change({ message: ltr(message) }), m.activity_describing_change({ message: ltr(message) }))
        : vcs("git-commit", RANK.vcsWrite, m.activity_described_the_change(), m.activity_describing_the_change());
    }
    case "commit":
    case "ci":
      return committed(commitMessage(rest, segment, at, context));
    case "new":
      return vcs("git-commit", RANK.vcsWrite, m.activity_started_change(), m.activity_starting_change());
    case "squash":
      return vcs("git-merge", RANK.vcsWrite, m.activity_squashed_changes(), m.activity_squashing_changes());
    case "edit":
      return positional[0] ? switched(positional[0]) : null;
    case "rebase":
      return rebased(valueOf("-d", "--destination", "-o", "--onto"));
    case "restore":
      return restored(positional);
    case "abandon":
      return vcs("trash", RANK.vcsWrite, m.activity_abandoned_change(), m.activity_abandoning_change());
    case "git":
      if (rest[0] === "push") return pushed(valueOf("-b", "--bookmark", "-c", "--change"));
      if (rest[0] === "fetch") return pulled();
      return null;
    case "bookmark":
    case "b": {
      const target = rest.slice(1).find((arg, at, all) => !arg.startsWith("-") && !["-r", "--revision", "--to"].includes(all[at - 1]));
      if (["set", "s", "create", "c", "move", "m"].includes(rest[0]) && target) {
        return vcs("bookmark", RANK.vcsWrite, m.activity_set_bookmark({ target: ltr(target) }), m.activity_setting_bookmark({ target: ltr(target) }));
      }
      if (rest[0] === "list" || rest[0] === "l") return vcs("git-branch", RANK.vcsRead, m.activity_listed_branches(), m.activity_listing_branches());
      return null;
    }
    default:
      return null;
  }
}

function githubStep({ name, args }: ShellInvocation): Step | null {
  if (name !== "gh") return null;
  const [area, action] = args;
  const number = args.slice(2).find((arg) => /^#?\d+$/.test(arg) || /\/(?:pull|issues)\/\d+/.test(arg));
  const ref = number ? `#${number.replace(/^#/, "").replace(/^.*\/(?:pull|issues)\//, "")}` : null;
  const gh = (icon: Glyph, rank: number, label: string, progress: string) => plain("command", icon, rank, label, progress);
  if ((area === "pr" || area === "issue") && (action === "list" || action === "status")) {
    return gh("globe", RANK.github, m.activity_queried_github(), m.activity_querying_github());
  }
  if (area === "pr") {
    switch (action) {
      case "view":
      case "diff":
        return ref
          ? gh("pull-request", RANK.github, m.activity_viewed_pull_request({ target: ltr(ref) }), m.activity_viewing_pull_request({ target: ltr(ref) }))
          : gh("pull-request", RANK.github, m.activity_viewed_the_pull_request(), m.activity_viewing_the_pull_request());
      case "checks":
        return gh("ci", RANK.github, m.activity_checked_ci(), m.activity_checking_ci());
      case "create": {
        const title = messageLine(optionValue(args, "-t", "--title"));
        return title
          ? gh("pull-request", RANK.vcsWrite, m.activity_opened_pull_request({ title: ltr(title) }), m.activity_opening_pull_request({ title: ltr(title) }))
          : gh("pull-request", RANK.vcsWrite, m.activity_opened_a_pull_request(), m.activity_opening_a_pull_request());
      }
      case "merge":
        return gh("git-merge", RANK.vcsWrite, m.activity_merged_pull_request(), m.activity_merging_pull_request());
      case "comment":
      case "review":
        return gh("message", RANK.github, m.activity_commented_on_github(), m.activity_commenting_on_github());
      default:
        return null;
    }
  }
  if (area === "issue") {
    if (action === "view") {
      return ref
        ? gh("issue", RANK.github, m.activity_viewed_issue({ target: ltr(ref) }), m.activity_viewing_issue({ target: ltr(ref) }))
        : null;
    }
    if (action === "comment") return gh("message", RANK.github, m.activity_commented_on_github(), m.activity_commenting_on_github());
    return null;
  }
  if (area === "run" || area === "workflow") return gh("ci", RANK.github, m.activity_checked_ci(), m.activity_checking_ci());
  if (["api", "search", "release", "repo", "label"].includes(area)) {
    return gh("globe", RANK.github, m.activity_queried_github(), m.activity_querying_github());
  }
  return null;
}

/** Tests, type checks, lints, formatting, builds and installs, by the tool or the package script that runs them. */
function devStep(invocation: ShellInvocation): Step | null {
  const { name, args } = invocation;
  const tests = () => plain("command", "tests", RANK.test, m.chat_panel_ran_tests(), m.activity_running_tests());
  const types = () => plain("command", "types", RANK.check, m.chat_panel_checked_types(), m.activity_checking_types());
  const lint = () => plain("command", "lint", RANK.check, m.chat_panel_checked_code_style(), m.activity_checking_code_style());
  const format = () => plain("command", "format", RANK.format, m.activity_formatted_code(), m.activity_formatting_code());
  const build = () => plain("command", "build", RANK.build, m.chat_panel_built_the_project(), m.activity_building_the_project());
  const install = (packages: string[] = []): Step => {
    const names = packages.filter((arg) => !arg.startsWith("-") && !/[/\\]/.test(arg));
    return names.length > 0
      ? { kind: "command", icon: "package", rank: RANK.install, phrase: phrases.installed, targets: names.map((value) => ({ name: value })) }
      : plain("command", "package", RANK.install, m.activity_installed_dependencies(), m.activity_installing_dependencies());
  };
  /** A package script by its name: `test`, `test:unit`, `typecheck`, `lint:i18n`. */
  const script = (value: string | undefined): Step | null => {
    const base = value?.split(":")[0];
    if (base === "test" || base === "e2e" || base === "spec") return tests();
    if (base === "typecheck" || base === "check-types" || base === "tsc") return types();
    if (base === "lint") return lint();
    if (base === "format" || base === "fmt") return format();
    if (base === "build") return build();
    return null;
  };

  switch (name) {
    case "cargo": {
      const sub = args.find((arg) => !arg.startsWith("-") && !arg.startsWith("+"));
      if (sub === "test" || sub === "nextest" || sub === "t") return tests();
      if (sub === "check" || sub === "c") return types();
      if (sub === "clippy") return lint();
      if (sub === "fmt") return args.includes("--check") ? lint() : format();
      if (sub === "build" || sub === "b") return build();
      if (sub === "add" || sub === "install") return install(args.slice(args.indexOf(sub) + 1));
      if (sub === "clean") return plain("command", "trash", RANK.files, m.activity_cleaned_build(), m.activity_cleaning_build());
      return null;
    }
    case "pnpm":
    case "npm":
    case "yarn":
    case "bun": {
      const words = args.filter((arg, index) => !arg.startsWith("-") && !["--filter", "-F", "-C", "--dir", "--prefix", "-w"].includes(args[index - 1]));
      const [sub, next] = words;
      if (["install", "i", "ci"].includes(sub) || (name === "yarn" && words.length === 0)) return install();
      if (sub === "add") return install(words.slice(1));
      if (sub === "run" || sub === "run-script") return script(next);
      // `pnpm --dir ui exec vite build` runs a tool of its own.
      if (sub === "exec" || sub === "dlx" || sub === "x") return next ? devStep({ name: next, path: next, args: words.slice(2) }) : null;
      if (name === "bun" && sub && /\.[mc]?[jt]sx?$/.test(sub)) return null;
      return script(sub);
    }
    case "node":
      return args.includes("--test") ? tests() : null;
    case "pytest":
    case "vitest":
    case "jest":
    case "mocha":
    case "ava":
    case "tox":
    case "nox":
      return tests();
    case "playwright":
      return args[0] === "test" ? tests() : args[0] === "install" ? install() : null;
    case "vite":
    case "next":
    case "astro":
      return args[0] === "build" ? build() : null;
    case "go":
      if (args[0] === "test") return tests();
      if (args[0] === "build") return build();
      if (args[0] === "vet") return lint();
      if (args[0] === "get" || (args[0] === "mod" && args[1] === "tidy")) return install();
      return null;
    case "tsc":
    case "vue-tsc":
    case "mypy":
    case "pyright":
    case "basedpyright":
      return args.includes("-b") || args.includes("--build") ? build() : types();
    case "eslint":
    case "flake8":
    case "pylint":
    case "shellcheck":
    case "golangci-lint":
    case "biome":
    case "oxlint":
    case "stylelint":
      return lint();
    case "ruff":
      return args[0] === "format" ? (args.includes("--check") ? lint() : format()) : lint();
    case "prettier":
    case "black":
    case "isort":
    case "gofmt":
    case "rustfmt":
    case "clang-format":
    case "dprint":
      return args.includes("--check") || args.includes("-l") ? lint() : format();
    case "make":
    case "just":
    case "ninja": {
      const target = args.find((arg) => !arg.startsWith("-") && !arg.includes("="));
      return script(target) ?? (target ? null : build());
    }
    case "xcodebuild":
    case "swift":
      return args.includes("test") ? tests() : args.includes("build") || name === "xcodebuild" ? build() : null;
    case "docker":
      return args[0] === "build" ? build() : null;
    case "pip":
    case "pip3":
      return args[0] === "install" ? install(args.slice(1)) : null;
    case "uv":
      if (args[0] === "sync" || args[0] === "lock") return install();
      if (args[0] === "add") return install(args.slice(1));
      if (args[0] === "pip" && args[1] === "install") return install(args.slice(2));
      return null;
    case "poetry":
      return args[0] === "install" ? install() : args[0] === "add" ? install(args.slice(1)) : null;
    case "brew":
    case "apt":
    case "apt-get":
      return args[0] === "install" ? install(args.slice(1)) : null;
    case "python":
    case "python3": {
      const module = args[args.indexOf("-m") + 1];
      if (!args.includes("-m")) return null;
      if (module === "pytest" || module === "unittest") return tests();
      if (module === "mypy") return types();
      if (module === "pip" && args.includes("install")) return install(args.slice(args.indexOf("install") + 1));
      if (module === "ruff" || module === "black") return lint();
      return null;
    }
    default:
      return null;
  }
}

const INLINE_LANGUAGE: Record<string, string> = {
  python: "Python", python3: "Python", node: "Node.js", deno: "Deno", bun: "Bun", ruby: "Ruby", perl: "Perl",
  bash: "Bash", sh: "shell", zsh: "zsh", swift: "Swift",
};

/** Interpreter options that take the next word, which is never the script. */
const INTERPRETER_VALUES: Record<string, Set<string>> = {
  python: new Set(["-X", "-W", "-Q"]),
  node: new Set(["--import", "--require", "-r", "--loader", "--experimental-loader", "--env-file", "-C", "--conditions"]),
  ruby: new Set(["-I", "-r", "-C"]),
  perl: new Set(["-I", "-M", "-m"]),
};

/** Programs run from a file or as inline code; inline Python that writes files reads as editing them. */
function scriptStep(invocation: ShellInvocation, segment: ShellSegment, at: number, context: StepContext): Step | null {
  const { name, path, args } = invocation;
  const interpreter = /^python[\d.]*$/.test(name) ? "python" : name;
  const language = INLINE_LANGUAGE[interpreter];
  const ran = (file: string): Step => {
    const target = fileTarget(file, at, context);
    return { kind: "command", icon: "play", rank: RANK.script, phrase: phrases.ran, targets: [target], runs: target.file?.path };
  };

  if (language) {
    const inline = args.findIndex((arg) => arg === "-c" || arg === "-e" || arg === "--eval" || arg === "-");
    const valued = INTERPRETER_VALUES[interpreter] ?? new Set<string>();
    const positional = args.some((arg, index) => !arg.startsWith("-") && !valued.has(args[index - 1]));
    const code = inline >= 0
      ? args[inline] === "-" ? segment.heredoc : args[inline + 1]
      : segment.heredoc !== undefined && !positional ? segment.heredoc : undefined;
    if (code !== undefined) {
      // `bash -c "<script>"` runs one more script: describe that instead.
      if ((interpreter === "bash" || interpreter === "sh" || interpreter === "zsh") && args[inline] === "-c") {
        const inner = shellSteps(code, [], null);
        if (inner.length > 0) return inner.reduce((best, step) => step.rank > best.rank ? step : best);
      }
      if (interpreter === "python") {
        const written = pythonWrites(code).map((file) => fileTarget(file, at, context));
        if (written.length > 0) return { kind: "edit", icon: "file-pen", rank: RANK.edit, phrase: phrases.edited, targets: written };
      }
      return plain("command", "code", RANK.script, m.activity_ran_inline_script({ language }), m.activity_running_inline_script({ language }));
    }
    if (interpreter === "python" && args.includes("-m")) {
      const module = args[args.indexOf("-m") + 1];
      if (!module || ["http.server", "json.tool", "venv", "pip"].includes(module)) return null;
      return { kind: "command", icon: "play", rank: RANK.script, phrase: phrases.ran, targets: [{ name: module }] };
    }
    const file = args.find((arg, index) => !arg.startsWith("-") && !valued.has(args[index - 1]));
    if (file && fileLike(file)) return ran(file);
    return null;
  }
  if (["tsx", "ts-node", "deno"].includes(name)) {
    const file = args.find((arg) => !arg.startsWith("-") && arg !== "run");
    return file && fileLike(file) ? ran(file) : null;
  }
  // A script run by its path: `./scripts/check.sh`, `.venv/bin/train`.
  if (path.includes("/") && fileLike(path) && !path.startsWith("/usr/") && !path.startsWith("/bin/") && !path.startsWith("/opt/homebrew/")) {
    return ran(path);
  }
  return null;
}

/** The files inline Python writes with `Path(…).write_text`, `open(…, "w")` or a variable holding such a path. */
function pythonWrites(code: string): string[] {
  // A string literal; its text is in whichever of the two groups matched.
  const literal = String.raw`(?:"([^"\n]+)"|'([^'\n]+)')`;
  const text = (match: RegExpMatchArray, group: number) => match[group] ?? match[group + 1];
  // What each name may hold: a literal path, `Path(<literal>)`, the values a loop gives it, or what calls pass a function's first parameter.
  const values = new Map<string, string[]>();
  const hold = (name: string, held: string[]) => {
    const known = values.get(name);
    if (known) known.push(...held);
    else values.set(name, [...held]);
  };
  for (const match of code.matchAll(new RegExp(String.raw`(?:^|[\s;(])([A-Za-z_]\w*)\s*=\s*(?:(?:pathlib\.)?Path\(\s*)?${literal}`, "gm"))) {
    hold(match[1], [text(match, 2)]);
  }
  for (const match of code.matchAll(/\bfor\s+([A-Za-z_]\w*)\s+in\s+[[(]([^\])]*)[\])]/g)) {
    hold(match[1], [...match[2].matchAll(new RegExp(literal, "g"))].map((value) => text(value, 1)));
  }
  const calls = new Map<string, string[]>();
  for (const call of code.matchAll(new RegExp(String.raw`\b([A-Za-z_]\w*)\(\s*${literal}`, "g"))) {
    const known = calls.get(call[1]);
    if (known) known.push(text(call, 2));
    else calls.set(call[1], [text(call, 2)]);
  }
  for (const match of code.matchAll(/\bdef\s+([A-Za-z_]\w*)\s*\(\s*([A-Za-z_]\w*)/g)) hold(match[2], calls.get(match[1]) ?? []);

  const writtenTo = new Set<string>();
  const written = (expression: string) => writtenTo.add(expression);
  const expression = String.raw`((?:"[^"\n]+")|(?:'[^'\n]+')|[A-Za-z_]\w*)`;
  for (const match of code.matchAll(new RegExp(String.raw`\bopen\(\s*${expression}\s*,\s*(?:mode\s*=\s*)?["'][wax]`, "g"))) written(match[1]);
  for (const match of code.matchAll(new RegExp(String.raw`(?:pathlib\.)?Path\(\s*${expression}\s*\)\s*\.write_(?:text|bytes)\(`, "g"))) written(match[1]);
  for (const match of code.matchAll(/\b([A-Za-z_]\w*)\.write_(?:text|bytes)\(/g)) written(match[1]);

  const files = new Set<string>();
  for (const expression of writtenTo) {
    const quoted = new RegExp(`^${literal}$`).exec(expression);
    for (const file of quoted ? [text(quoted, 1)] : values.get(expression) ?? []) {
      // Only what reads as a path: a name with an extension or a directory, no spaces or markup.
      if (/^[\w.~/@+-]+$/.test(file) && (/\.[A-Za-z0-9]+$/.test(file) || file.includes("/"))) files.add(file);
    }
  }
  return [...files];
}

function systemStep(invocation: ShellInvocation, at: number, context: StepContext): Step | null {
  const { name, args } = invocation;
  const system = (icon: Glyph, rank: number, label: string, progress: string) => plain("command", icon, rank, label, progress);
  switch (name) {
    case "mkdir": {
      const targets = args.filter((arg) => !arg.startsWith("-")).flatMap((arg) => plainTarget(arg) ?? []);
      return targets.length > 0 ? { kind: "command", icon: "folder-plus", rank: RANK.files, phrase: phrases.createdFolder, targets: targets.map((target) => ({ name: baseName(target) })) } : null;
    }
    case "rm":
    case "rmdir":
    case "trash": {
      const targets = args.filter((arg) => !arg.startsWith("-")).flatMap((arg) => plainTarget(arg) ?? []);
      return targets.length > 0 ? { kind: "command", icon: "trash", rank: RANK.files, phrase: phrases.deleted, targets: targets.map((target) => ({ name: baseName(target) })) } : null;
    }
    case "mv":
    case "cp": {
      const paths = args.filter((arg) => !arg.startsWith("-"));
      const source = plainTarget(paths[0]);
      if (!source || paths.length < 2) return null;
      return { kind: "command", icon: name === "mv" ? "move" : "copy", rank: RANK.files, phrase: name === "mv" ? phrases.moved : phrases.copied, targets: [{ name: baseName(source) }] };
    }
    case "curl":
    case "wget":
    case "http":
    case "https":
    case "xh": {
      const url = args.find((arg) => /^https?:\/\//.test(arg) || /^(?:localhost|127\.0\.0\.1)[:/]/.test(arg));
      if (!url) return null;
      const target = withoutVariables(url).replace(/^https?:\/\//, "").replace(/[?#].*$/, "").replace(/\/$/, "");
      return { kind: "web", icon: "globe", rank: RANK.web, phrase: phrases.fetched, targets: [{ name: target }] };
    }
    case "open":
    case "xdg-open": {
      const target = args.find((arg) => !arg.startsWith("-"));
      return target ? system(/^https?:/.test(target) ? "globe" : "external", RANK.web, m.activity_opened_target({ target: ltr(target) }), m.activity_opening_target({ target: ltr(target) })) : null;
    }
    case "ssh": {
      const values = new Set(["-o", "-i", "-p", "-l", "-F", "-J", "-L", "-R", "-D", "-W", "-E", "-b", "-c", "-m", "-O", "-Q", "-S", "-w", "-B", "-I"]);
      let index = 0;
      while (args[index]?.startsWith("-")) index += values.has(args[index]) ? 2 : 1;
      const host = args[index]?.replace(/^.*@/, "");
      if (!host) return null;
      const remote = args.slice(index + 1).join(" ");
      if (!remote) return system("server", RANK.remote, m.activity_connected_to({ host: ltr(host) }), m.activity_connecting_to({ host: ltr(host) }));
      const command = remote.length > 80 ? `${remote.slice(0, 79)}…` : remote;
      return system("server", RANK.remote, m.activity_ran_on_host({ command: ltr(command), host: ltr(host) }), m.activity_running_on_host({ command: ltr(command), host: ltr(host) }));
    }
    case "scp":
    case "rsync": {
      const paths = args.filter((arg) => !arg.startsWith("-"));
      const source = plainTarget(paths[0]);
      return source ? { kind: "command", icon: "copy", rank: RANK.remote, phrase: phrases.copied, targets: [{ name: baseName(source) }] } : null;
    }
    case "sqlite3":
    case "duckdb": {
      const database = args.find((arg) => !arg.startsWith("-"));
      const target = database?.replace(/^file:/, "").replace(/\?.*$/, "");
      return target
        ? { kind: "command", icon: "database", rank: RANK.data, phrase: phrases.queried, targets: [{ name: baseName(target) }] }
        : null;
    }
    case "psql":
    case "mysql":
      return system("database", RANK.data, m.activity_queried_database(), m.activity_querying_database());
    case "ps":
    case "pgrep":
    case "top":
    case "htop":
    case "lsof":
      return system("activity", RANK.info, m.activity_checked_processes(), m.activity_checking_processes());
    case "pkill":
    case "killall": {
      const pattern = args.filter((arg) => !arg.startsWith("-")).at(-1);
      return pattern
        ? { kind: "command", icon: "stop", rank: RANK.stop, phrase: phrases.stopped, targets: [{ name: pattern }] }
        : system("stop", RANK.stop, m.activity_stopped_a_process(), m.activity_stopping_a_process());
    }
    case "kill":
      return system("stop", RANK.stop, m.activity_stopped_a_process(), m.activity_stopping_a_process());
    case "which":
    case "whereis":
    case "type":
    case "command": {
      const target = args.find((arg) => !arg.startsWith("-"));
      return target ? { kind: "command", icon: "info", rank: RANK.info, phrase: phrases.lookedUp, targets: [{ name: target }] } : null;
    }
    case "uname":
    case "sw_vers":
    case "sysctl":
    case "nproc":
    case "hostname":
    case "whoami":
    case "id":
    case "system_profiler":
    case "lscpu":
    case "free":
    case "vm_stat":
    case "uptime":
      return system("cpu", RANK.info, m.activity_checked_system(), m.activity_checking_system());
    case "nvidia-smi":
    case "rocm-smi":
      return system("cpu", RANK.info, m.activity_checked_gpus(), m.activity_checking_gpus());
    case "df":
    case "du":
      return system("disk", RANK.info, m.activity_checked_disk_usage(), m.activity_checking_disk_usage());
    case "env":
    case "printenv":
      return args.length === 0 || args.every((arg) => !arg.includes("="))
        ? system("variable", RANK.info, m.activity_checked_environment(), m.activity_checking_environment())
        : null;
    case "wc": {
      const targets = args.flatMap((arg) => looseTarget(arg, at, context) ?? []);
      return targets.length > 0 ? { kind: "read", icon: "file-text", rank: RANK.read, phrase: phrases.counted, targets } : null;
    }
    default:
      return null;
  }
}

/** Seconds in a `sleep` argument: `240`, `1.5`, `5m`, `1h`. */
function durationSeconds(value: string | undefined): number | null {
  const match = /^(\d+(?:\.\d+)?)([smhd]?)$/.exec(value ?? "");
  if (!match) return null;
  const unit = { "": 1, s: 1, m: 60, h: 3600, d: 86_400 }[match[2]] ?? 1;
  return Number(match[1]) * unit;
}

/** The call as one row: steps with a shared phrase merged, a script written then run as one, the most telling step first. */
export function summarizeSteps(input: Step[]): ShellSummary | null {
  // Pair each write with its run before merging, which keeps only the first write's path.
  const runs = new Set(input.flatMap((step) => step.runs ? [step.runs] : []));
  const ran = new Set(input.flatMap((step) => step.writes && runs.has(step.writes) ? [step.writes] : []));
  const steps = mergeSteps(input.flatMap((step): Step[] => {
    if (step.writes && ran.has(step.writes)) return [];
    if (step.runs && ran.has(step.runs)) return [{ ...step, rank: RANK.script, phrase: phrases.wroteAndRan }];
    return [step];
  }));

  const described = steps.map((step) => ({ step, ...describe(step) }));
  const unique = described.filter((entry, index) => described.findIndex((other) => other.label === entry.label) === index);
  if (unique.length === 0) return null;
  const headline = unique.reduce((best, entry) => entry.step.rank > best.step.rank ? entry : best);
  return {
    kind: headline.step.kind,
    icon: headline.step.icon,
    label: headline.label,
    progress: headline.progress,
    files: (headline.step.targets ?? []).flatMap((target) => target.file ? [target.file] : []),
    more: unique.filter((entry) => entry !== headline).map((entry) => entry.label),
  };
}

function mergeSteps(steps: Step[]): Step[] {
  const merged: Step[] = [];
  for (const step of steps) {
    const same = step.phrase && step.targets && merged.find((other) => other.phrase === step.phrase);
    if (!same) {
      merged.push(step);
      continue;
    }
    const key = (target: Target) => target.file ? `${target.file.path}@${target.file.ref ?? ""}` : target.name;
    const known = new Set(same.targets?.map(key));
    same.targets = [...same.targets ?? [], ...step.targets!.filter((target) => !known.has(key(target)))];
  }
  return merged;
}

function describe(step: Step): { label: string; progress: string } {
  if (step.phrase && step.targets?.length) {
    const target = listNames(step.targets.map((value) => ltr(value.name)));
    return { label: step.phrase.done(target), progress: step.phrase.doing(target) };
  }
  return { label: step.label ?? "", progress: step.progress ?? step.label ?? "" };
}
