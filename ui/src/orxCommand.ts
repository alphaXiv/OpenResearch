export type LitSource = "alphaxiv" | "openalex" | "biorxiv";

export type OrxLitCall =
  | { kind: "paper"; source: LitSource; id?: string }
  | {
      kind: "discover";
      source: LitSource;
      strategy: "keyword" | "embedding" | "openalex" | "biorxiv";
      query?: string;
    };

export function containsShellGlob(value: string): boolean {
  return ["*", "?", "[", "]", "{", "}"].some((token) => value.includes(token));
}

function asSource(value: string | undefined): LitSource | undefined {
  return value === "alphaxiv" || value === "openalex" || value === "biorxiv"
    ? value
    : undefined;
}

function detectPaperSource(id: string): LitSource {
  const value = id.trim();
  const lower = value.toLowerCase();
  if (lower.includes("biorxiv.org")) return "biorxiv";
  if (lower.includes("openalex.org")) return "openalex";
  const doi = value.match(/10\.\d+\/\S+/);
  if (doi) return doi[0].startsWith("10.1101/") ? "biorxiv" : "openalex";
  const last = value.split("/").pop() ?? "";
  if (/^W\d+$/i.test(last)) return "openalex";
  return "alphaxiv";
}

/** Shell-style tokens up to the first operator, including Codex's quoted argv display. */
export function shellWords(input: string): string[] {
  const words: string[] = [];
  let word = "";
  let hasWord = false;
  let quote: '"' | "'" | null = null;

  const push = () => {
    if (hasWord) words.push(word);
    word = "";
    hasWord = false;
  };

  for (let index = 0; index < input.length; index++) {
    const char = input[index];
    if (char === "\\" && quote !== "'") {
      const next = input[index + 1];
      const escapesInDoubleQuotes = next !== undefined && ["$", "`", '"', "\\", "\n"].includes(next);
      if (quote === '"' && !escapesInDoubleQuotes) {
        word += char;
        hasWord = true;
        continue;
      }
      if (next === undefined) {
        word += char;
        hasWord = true;
        continue;
      }
      index++;
      if (next !== "\n") {
        word += next;
        hasWord = true;
      }
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      else word += char;
      hasWord = true;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      hasWord = true;
      continue;
    }
    if (char === "|" || char === ";" || char === ">" || char === "&") break;
    if (/\s/.test(char)) push();
    else {
      word += char;
      hasWord = true;
    }
  }
  push();
  return words;
}

/** Remove quotes only when they make the entire body one shell word. */
export function unwrapShellBody(input: string): string {
  const first = input[0];
  if ((first === '"' || first === "'") && input.at(-1) === first) {
    const words = shellWords(input);
    if (words.length === 1) return words[0];
  }
  return input;
}

/** The argv after an `orx` executable in shell command position. */
export function orxArgvFromTokens(tokens: readonly string[]): string[] | null {
  let index = 0;
  while (["do", "then", "else", "if", "while", "until"].includes(tokens[index])) index++;
  while (/^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[index] ?? "")) index++;
  if (tokens[index] === "env") {
    index++;
    while (tokens[index]?.startsWith("-") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[index] ?? "")) {
      index++;
    }
  }
  if (tokens[index] === "command") {
    index++;
    if (["-v", "-V"].includes(tokens[index])) return null;
    while (tokens[index]?.startsWith("-")) index++;
  }
  if (tokens[index]?.split("/").pop() !== "orx") return null;
  return tokens.slice(index + 1);
}

export function orxArgv(command: string | readonly string[]): string[] | null {
  return orxArgvFromTokens(typeof command === "string" ? shellWords(command) : command);
}

export function shellWrapperBody(argv: readonly string[]): string | null {
  const shell = argv[0]?.split("/").pop();
  if (!shell || !["sh", "bash", "zsh"].includes(shell) || argv[1] !== "-lc") return null;
  return argv[2] ?? null;
}

/** Match an `orx` argv prefix regardless of whether Codex quoted every token. */
export function orxArgsMatch(command: string | readonly string[], args: string): boolean {
  const argv = orxArgv(command);
  if (argv === null) return false;
  // Each `\s+`-separated fragment is one argv-token regex, never a literal space.
  const patterns = args.split("\\s+");
  return patterns.every(
    (pattern, index) => argv[index] !== undefined && new RegExp(`^(?:${pattern})$`, "i").test(argv[index]),
  );
}

/** Parse the first literature command from a shell segment. */
export function parseOrxLit(command: string | readonly string[]): OrxLitCall | null {
  const argv = orxArgv(command);
  if (!argv) return null;
  const kind = argv[0];
  if (kind !== "paper" && kind !== "discover") return null;

  let source: LitSource | undefined;
  const positionals: string[] = [];
  const valueFlags = new Set([
    "--limit",
    "--published-after",
    "--published-before",
    "--prioritize",
  ]);
  for (let index = 1; index < argv.length; index++) {
    const token = argv[index];
    if (token === "--source") {
      source = asSource(argv[++index]);
      continue;
    }
    if (token.startsWith("--source=")) {
      source = asSource(token.slice("--source=".length));
      continue;
    }
    if (valueFlags.has(token)) {
      if (!argv[index + 1]?.startsWith("--")) index++;
      continue;
    }
    if (token.startsWith("--")) continue;
    positionals.push(token);
  }

  if (kind === "paper") {
    const id = positionals[0];
    return {
      kind,
      source: source ?? (id ? detectPaperSource(id) : "alphaxiv"),
      id,
    };
  }

  const strategy = positionals[0];
  if (
    strategy !== "keyword" &&
    strategy !== "embedding" &&
    strategy !== "openalex" &&
    strategy !== "biorxiv"
  ) return null;
  const discoverSource = strategy === "openalex" || strategy === "biorxiv"
    ? strategy
    : "alphaxiv";
  return { kind, source: discoverSource, strategy, query: positionals[1] };
}
