import { shellWords, unwrapShellBody } from "../orxCommand";

/** One simple command of a shell script, split at `;`, `&&`, `||`, `|`, `&`, parentheses and newlines. */
export interface ShellSegment {
  /** The command as written, quotes kept. */
  raw: string;
  /** Its unquoted characters, where operators and redirections show. */
  code: string;
  /** Reads the previous command's output through a pipe. */
  piped: boolean;
  /** Runs inside `$(…)` or backticks to compute a value. */
  nested: boolean;
  /** The body of the here-document it opens. */
  heredoc?: string;
}

/** A command's executable and arguments, past keywords, assignments and wrappers like `env` or `timeout`. */
export interface ShellInvocation {
  name: string;
  /** The executable as written, for scripts run by path. */
  path: string;
  args: string[];
}

/** The script a harness ran: Codex reports `/bin/zsh -lc '<script>'`, and that wrapper is plumbing. */
export function shellBody(command: string): string {
  const trimmed = command.trim();
  const wrapped = trimmed.match(/^\/bin\/(?:ba|z)?sh\s+-lc\s+([\s\S]+)$/);
  return unwrapShellBody((wrapped?.[1] ?? trimmed).trim());
}

/** The script with each here-document's body cut out, in the order they open. */
export function extractHeredocs(script: string): { text: string; bodies: string[] } {
  const lines = script.split("\n");
  const kept: string[] = [];
  const bodies: string[] = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    kept.push(line);
    // `<<` inside quotes or a comment is text; a quoted delimiter right after `<<` stays.
    const code = line
      .replace(/(<<-?\s*)?('[^']*'|"(?:\\.|[^"\\])*")/g, (quoted, opener?: string) => opener ? quoted : '""')
      .replace(/(^|\s)#.*$/, "$1");
    const marker = /(?:^|[^<])<<(-?)\s*(?:'([^'\n]+)'|"([^"\n]+)"|\\?([A-Za-z_][^\s;&|<>()'"]*))/.exec(code);
    if (!marker) continue;
    const [, stripTabs] = marker;
    const delimiter = marker[2] ?? marker[3] ?? marker[4];
    // Unterminated, `<<` was a shift or text, not a here-document.
    const end = lines.findIndex((candidate, at) =>
      at > index && (stripTabs ? candidate.replace(/^\t+/, "") : candidate).trimEnd() === delimiter);
    if (end < 0) continue;
    bodies.push(lines.slice(index + 1, end).join("\n"));
    index = end;
  }
  return { text: kept.join("\n"), bodies };
}

/** A script on one line: here-document bodies dropped and runs of spaces collapsed, as labels quote it. */
export function meaningfulCommand(command: string): string {
  return extractHeredocs(shellBody(command)).text.replace(/[\t\r ]+/g, " ").trim();
}

/** The script with each variable a literal assignment set replaced by its value until the next assignment to it:
 * `ORX="$ORX_BIN_DIR/OpenResearch"; "$ORX" exp status` reads as running orx. */
export function withVariables(command: string): string {
  const assignment = /(?:^|[\s;&|(])(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(?:"([^"]*)"|'([^']*)'|([^\s;&|()"'`]*))(?=[\s;&|)]|$)/g;
  let result = command;
  let from = 0;
  // Each substitution rescans the rest of the script; real scripts set a handful, and none should grow it much.
  for (let count = 0; count < 64 && result.length <= command.length * 2 + 4096; count++) {
    assignment.lastIndex = from;
    const match = assignment.exec(result);
    if (!match) break;
    const [, name] = match;
    const value = match[2] ?? match[3] ?? match[4] ?? "";
    const end = match.index + match[0].length;
    from = end;
    const reference = `\\$(?:\\{${name}\\}|${name}(?![A-Za-z0-9_]))`;
    // Shell syntax in a value would read as commands once substituted; a value naming itself never settles.
    if (/[`"'\\;&|<>()\n]|\$\(/.test(value) || new RegExp(reference).test(value)) continue;
    const next = new RegExp(`(?:^|[\\s;&|(])(?:export\\s+)?${name}=`).exec(result.slice(end));
    const until = next ? end + next.index : result.length;
    const scoped = result.slice(end, until).replace(new RegExp(`(")?${reference}(")?`, "g"), (use, open?: string, close?: string) =>
      // Unquoted, a value with spaces would split into words.
      /\s/.test(value) && !(open && close) ? use : `${open ?? ""}${value}${close ?? ""}`);
    result = result.slice(0, end) + scoped + result.slice(until);
  }
  return result;
}

export function shellSegments(command: string, bodies: string[] = []): ShellSegment[] {
  const segments = splitSegments(command, 0);
  let next = 0;
  for (const segment of segments) {
    if (next < bodies.length && /(?:^|[^<])<<(?!<)/.test(segment.code)) segment.heredoc = bodies[next++];
  }
  return segments;
}

/** Substitutions nest no deeper than this before their text is taken as is. */
const MAX_NESTING = 8;

function splitSegments(command: string, depth: number): ShellSegment[] {
  const segments: ShellSegment[] = [];
  let raw = "";
  let code = "";
  let quote: "\"" | "'" | null = null;
  let escaped = false;
  let piped = false;
  let nextPiped = false;
  let groups = 0;
  const push = () => {
    if (raw.trim() || code.trim()) {
      segments.push({ raw: raw.trim(), code: code.trim(), piped, nested: false });
    }
    raw = "";
    code = "";
  };
  const scanSubstitution = (start: number): number => {
    let depth = 1;
    let nestedQuote: "\"" | "'" | null = null;
    let nestedEscaped = false;
    for (let index = start; index < command.length; index++) {
      const char = command[index];
      if (nestedEscaped) {
        nestedEscaped = false;
        continue;
      }
      if (char === "\\" && nestedQuote !== "'") {
        nestedEscaped = true;
        continue;
      }
      if (nestedQuote) {
        if (char === nestedQuote) nestedQuote = null;
        continue;
      }
      if (char === "\"" || char === "'") {
        nestedQuote = char;
        continue;
      }
      if (char === "(") depth++;
      if (char === ")" && --depth === 0) return index;
    }
    return command.length - 1;
  };
  const scanBackticks = (start: number): number => {
    let nestedEscaped = false;
    for (let index = start; index < command.length; index++) {
      const char = command[index];
      if (nestedEscaped) {
        nestedEscaped = false;
        continue;
      }
      if (char === "\\") {
        nestedEscaped = true;
        continue;
      }
      if (char === "`") return index;
    }
    return command.length - 1;
  };

  for (let index = 0; index < command.length; index++) {
    const char = command[index];
    if (escaped) {
      raw += char;
      if (!quote) code += char;
      escaped = false;
      continue;
    }
    if (char === "\\" && quote !== "'") {
      raw += char;
      escaped = true;
      continue;
    }
    if (quote) {
      if (quote === "\"" && char === "$" && command[index + 1] === "(") {
        const end = scanSubstitution(index + 2);
        segments.push(...nestedSegments(command.slice(index + 2, end), depth));
        raw += command.slice(index, end + 1);
        index = end;
        continue;
      }
      if (quote === "\"" && char === "`") {
        const end = scanBackticks(index + 1);
        segments.push(...nestedSegments(command.slice(index + 1, end), depth));
        raw += command.slice(index, end + 1);
        index = end;
        continue;
      }
      raw += char;
      if (char === quote) quote = null;
      continue;
    }
    if (char === "\"" || char === "'") {
      raw += char;
      quote = char;
      continue;
    }
    if (char === "`") {
      const end = scanBackticks(index + 1);
      segments.push(...nestedSegments(command.slice(index + 1, end), depth));
      raw += command.slice(index, end + 1);
      index = end;
      continue;
    }
    if (char === "$" && command[index + 1] === "(") {
      const end = scanSubstitution(index + 2);
      segments.push(...nestedSegments(command.slice(index + 2, end), depth));
      raw += command.slice(index, end + 1);
      code += command.slice(index, end + 1);
      index = end;
      continue;
    }
    // `>&` and `&>` redirect; they do not end a command.
    if (char === "&" && (command[index - 1] === ">" || command[index + 1] === ">")) {
      raw += char;
      code += char;
      continue;
    }
    // Parentheses group commands only at a command's start; inside a word (`f()`, zsh's `*(N)`) they are text.
    if ((char === "(" && !/^(?:do|then|else|elif|if|while|until|!|time|\{)?$/.test(raw.trim())) || (char === ")" && groups === 0)) {
      raw += char;
      code += char;
      continue;
    }
    if (char === "(") groups++;
    if (char === ")") groups--;
    // A comment runs to the end of its line; operators in it are text.
    if (char === "#" && (raw === "" || /\s/.test(raw.at(-1) ?? ""))) {
      while (index + 1 < command.length && command[index + 1] !== "\n") index++;
      continue;
    }
    if (char === ";" || char === "|" || char === "&" || char === "(" || char === ")" || char === "\n") {
      if (char === "|") {
        const pipe = command[index - 1] !== "|" && command[index + 1] !== "|";
        if (command[index - 1] !== "|") nextPiped = pipe;
      } else if (char === "\n" ? raw.trim() !== "" : char !== "&" || command[index - 1] !== "|") {
        // A pipe at the end of a line feeds the command on the next.
        nextPiped = false;
      }
      push();
      piped = nextPiped;
      continue;
    }
    raw += char;
    code += char;
  }
  push();
  return segments;
}

function nestedSegments(command: string, depth: number): ShellSegment[] {
  if (depth >= MAX_NESTING) return [];
  return splitSegments(command, depth + 1).map((segment) => ({ ...segment, nested: true }));
}

/** Wrappers that run the command after their own options, with the options that take a value. */
const WRAPPERS = new Map<string, { values?: string[]; skipsOne?: boolean }>(Object.entries({
  env: { values: ["-u", "-C", "-S", "--unset", "--chdir"] },
  command: {},
  exec: { values: ["-a"] },
  builtin: {},
  nohup: {},
  sudo: { values: ["-u", "-g", "-h", "-p", "-C"] },
  time: {},
  nice: { values: ["-n"] },
  ionice: { values: ["-c", "-n"] },
  caffeinate: { values: ["-t", "-w"] },
  stdbuf: { values: ["-i", "-o", "-e"] },
  unbuffer: {},
  timeout: { values: ["-s", "-k", "--signal", "--kill-after"], skipsOne: true },
  gtimeout: { values: ["-s", "-k", "--signal", "--kill-after"], skipsOne: true },
  xargs: { values: ["-n", "-I", "-P", "-L", "-d", "-s", "-E", "-a", "--max-args", "--max-procs", "--delimiter", "--replace"] },
  npx: { values: ["-p", "--package"] },
  bunx: {},
  uvx: { values: ["--from", "--with", "--python", "-p"] },
  // `uv run` and the package runners are stripped by their own two-word rules below.
}));

const UV_RUN_VALUES = new Set([
  "--with", "--with-requirements", "--python", "-p", "--project", "--directory", "--group", "--extra",
  "--env-file", "--package", "--index", "--index-url", "--extra-index-url", "--only-group", "--no-group", "-w",
]);

/** The executable of a simple command, past keywords, assignments, comments and wrappers. */
export function shellInvocation(command: string): ShellInvocation | null {
  let words = shellWords(withoutSubstitutions(command));
  const comment = words.findIndex((word) => word.startsWith("#"));
  if (comment >= 0) words = words.slice(0, comment);
  words = withoutRedirections(words, command);
  let index = 0;
  for (;;) {
    while (["do", "then", "else", "elif", "if", "while", "until", "!", "{", "}"].includes(words[index])) index++;
    while (/^[A-Za-z_][A-Za-z0-9_]*\+?=/.test(words[index] ?? "")) index++;
    const word = words[index];
    if (word === undefined) return null;
    const name = executableName(word);
    const wrapper = WRAPPERS.get(name);
    if (wrapper) {
      if (name === "command" && ["-v", "-V"].includes(words[index + 1])) break;
      const start = index;
      index++;
      while (words[index]?.startsWith("-") || (name === "env" && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[index] ?? ""))) {
        if (words[index] === "--") {
          index++;
          break;
        }
        if (wrapper.values?.includes(words[index])) index++;
        index++;
      }
      if (wrapper.skipsOne) index++;
      // `env` alone prints the environment; it wraps nothing.
      if (words[index] === undefined) return { name, path: word, args: words.slice(start + 1) };
      continue;
    }
    if (name === "uv" && words[index + 1] === "run") {
      index += 2;
      while (words[index]?.startsWith("-")) {
        if (UV_RUN_VALUES.has(words[index])) index++;
        index++;
      }
      continue;
    }
    if (["pnpm", "poetry", "pipenv", "bundle", "mise"].includes(name) && ["exec", "dlx", "run"].includes(words[index + 1])
      && !(name === "pnpm" && words[index + 1] === "run")) {
      index += 2;
      if (words[index] === "--") index++;
      continue;
    }
    if (name === "conda" && words[index + 1] === "run") {
      index += 2;
      while (words[index]?.startsWith("-")) {
        if (["-n", "--name", "-p", "--prefix"].includes(words[index])) index++;
        index++;
      }
      continue;
    }
    break;
  }
  const path = words[index];
  if (path === undefined) return null;
  return { name: executableName(path), path, args: words.slice(index + 1) };
}

/** The command with each `$(…)` and backtick substitution reduced to one word, which segments describe on their own. */
function withoutSubstitutions(command: string): string {
  let result = "";
  let quote: "\"" | "'" | null = null;
  for (let index = 0; index < command.length; index++) {
    const char = command[index];
    if (char === "\\" && quote !== "'") {
      result += char + (command[index + 1] ?? "");
      index++;
      continue;
    }
    if (quote === "'") {
      if (char === "'") quote = null;
      result += char;
      continue;
    }
    if ((char === "$" && command[index + 1] === "(") || char === "`") {
      let depth = 0;
      let end = index;
      for (; end < command.length; end++) {
        const current = command[end];
        if (char === "`") {
          if (current === "`" && end > index) break;
          continue;
        }
        if (current === "(") depth++;
        else if (current === ")" && --depth === 0) break;
      }
      result += "$_";
      index = end;
      continue;
    }
    if (char === "\"" || char === "'") quote = quote === char ? null : quote ?? char;
    result += char;
  }
  return result;
}

function executableName(word: string): string {
  return word.split("/").pop() || word;
}

/** Arguments without input redirections or the descriptor a stopped `2>` left behind. */
function withoutRedirections(words: string[], command: string): string[] {
  const kept: string[] = [];
  for (let index = 0; index < words.length; index++) {
    const word = words[index];
    if (word === "<" || word === "<<" || word === "<<<") {
      index++;
      continue;
    }
    if (word.startsWith("<")) continue;
    kept.push(word);
  }
  if (/^\d$/.test(kept.at(-1) ?? "") && /\d>/.test(command)) kept.pop();
  return kept;
}

/** The file a command's output is written to with `>` or `>>`, skipping `/dev/null` and descriptors. */
export function redirectTarget(segment: ShellSegment): string | null {
  const { raw } = segment;
  let quote: "\"" | "'" | null = null;
  for (let index = 0; index < raw.length; index++) {
    const char = raw[index];
    if (char === "\\" && quote !== "'") {
      index++;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      continue;
    }
    if (char === "\"" || char === "'") {
      quote = char;
      continue;
    }
    if (char !== ">" || /[\d&<>]/.test(raw[index - 1] ?? "")) continue;
    const rest = raw.slice(index + (raw[index + 1] === ">" ? 2 : 1));
    if (rest.startsWith("&")) continue;
    const target = shellWords(rest)[0];
    if (target && !["/dev/null", "/dev/stderr", "/dev/stdout"].includes(target)) return target;
  }
  return null;
}

function resolvePosixPath(base: string, target: string): string | null {
  if (/[$`~]/.test(base) || /[$`~]/.test(target)) return null;
  const absolute = target.startsWith("/") || base.startsWith("/");
  const parts = target.startsWith("/") ? [] : base.split("/").filter(Boolean);
  for (const part of target.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length > 0 && parts[parts.length - 1] !== "..") parts.pop();
      else if (!absolute) parts.push(part);
      continue;
    }
    parts.push(part);
  }
  const resolved = `${absolute ? "/" : ""}${parts.join("/")}`;
  return resolved || (absolute ? "/" : null);
}

/** Where a path in segment `at` points, following the `cd`s before it from the call's working directory. */
export function segmentFilePath(
  target: string,
  segments: ShellSegment[],
  at: number,
  workdir: string | null,
): string | null {
  if (target.startsWith("/")) return target;
  let directory = workdir ?? "";
  for (let index = 0; index < at; index++) {
    const invocation = shellInvocation(segments[index].raw);
    if (invocation?.name !== "cd") continue;
    const next = invocation.args.find((arg) => !arg.startsWith("-"));
    if (!next) return null;
    const resolved = resolvePosixPath(directory, next);
    if (!resolved) return null;
    directory = resolved;
  }
  return directory ? resolvePosixPath(directory, target) : target;
}
