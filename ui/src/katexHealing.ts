// Ported from alphaXiv's rehype-katex-healing. Models emit math that KaTeX
// rejects (double-escaped commands, dangling escapes, unclosed braces), and
// streaming cuts it off mid-expression. Healing renders the longest valid
// prefix instead of an error box.
import katex from "katex";

export type KatexErrorHandling = "trim" | "escape";

const mathCodeLanguages = new Set(["language-math", "language-latex", "language-tex"]);
const DOUBLE_ESCAPED_BRACE_MACROS = [
  "bar", "bbox", "bcancel", "begin", "binom", "boxed", "cancel", "color", "dbinom", "ddot", "dfrac", "dot",
  "end", "fbox", "fcolorbox", "frac", "hat", "mathbb", "mathbf", "mathcal", "mathfrak", "mathit", "mathrm",
  "mathsf", "mathtt", "operatorname", "overbrace", "overline", "overset", "sqrt", "text", "textbf",
  "textcolor", "textit", "textrm", "textsf", "texttt", "tfrac", "tbinom", "tilde", "underbrace", "underline",
  "underset", "vec", "widehat", "widetilde", "xcancel",
].join("|");
const DOUBLE_ESCAPED_BARE_MACROS = [
  "alpha", "approx", "beta", "cdot", "cos", "delta", "det", "dim", "epsilon", "eta", "exp", "gamma", "geq",
  "infty", "iint", "iiint", "int", "iota", "kappa", "lambda", "ldots", "leq", "ln", "log", "mapsto", "max",
  "min", "mu", "nabla", "neq", "nu", "oint", "omega", "partial", "phi", "pi", "pm", "prod", "propto", "psi",
  "rho", "sigma", "sim", "sin", "sum", "tan", "tau", "theta", "times", "to", "triangleq", "upsilon",
  "varepsilon", "varphi", "vartheta", "xi", "zeta",
].join("|");

type KatexParseError = Error & { position?: number };
type KatexValidation = { ok: true } | { ok: false; error: KatexParseError | null };

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
  position?: unknown;
}

function classNames(element: HastNode): string[] {
  const value = element.properties?.["className"];
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function textContent(node: HastNode): string {
  return node.type === "text" ? node.value ?? "" : (node.children ?? []).map(textContent).join("");
}

function validate(value: string, displayMode: boolean): KatexValidation {
  try {
    katex.renderToString(value, { displayMode, throwOnError: true });
    return { ok: true };
  } catch (error) {
    return { ok: false, error: error instanceof Error && error.name === "ParseError" ? error : null };
  }
}

function trimDanglingEscape(value: string): string {
  const trailingBackslashes = value.match(/\\+$/)?.[0].length ?? 0;
  return trailingBackslashes % 2 === 0 ? value : value.slice(0, -1);
}

function normalizeDoubleEscapedCommands(value: string): string {
  return value
    .replace(new RegExp(`(?<!\\\\)\\\\\\\\(?=(?:${DOUBLE_ESCAPED_BRACE_MACROS})\\{)`, "g"), "\\")
    .replace(new RegExp(`(?<!\\\\)\\\\\\\\(?=(?:${DOUBLE_ESCAPED_BARE_MACROS})(?![A-Za-z]))`, "g"), "\\");
}

function isEscaped(value: string, index: number): boolean {
  let backslashes = 0;
  for (let cursor = index - 1; cursor >= 0 && value[cursor] === "\\"; cursor -= 1) backslashes += 1;
  return backslashes % 2 === 1;
}

function unclosedBraceCount(value: string): number {
  let balance = 0;
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    if ((char === "{" || char === "}") && isEscaped(value, index)) continue;
    if (char === "{") balance += 1;
    else if (char === "}" && balance > 0) balance -= 1;
  }
  return balance;
}

function autoCloseBraces(value: string, displayMode: boolean, error: KatexParseError): string | null {
  if (!/expected '\}'/i.test(error.message) || !/(end of input|EOF)/i.test(error.message)) return null;
  const unclosed = unclosedBraceCount(value);
  if (unclosed === 0) return null;
  const candidate = `${value}${"}".repeat(unclosed)}`;
  return validate(candidate, displayMode).ok ? candidate : null;
}

function stripTrailingFailingToken(value: string): string | null {
  const withoutDanglingEscape = trimDanglingEscape(value);
  if (withoutDanglingEscape !== value) return withoutDanglingEscape;
  if (value.endsWith("_") || value.endsWith("^")) return value.slice(0, -1);
  const trailingControlWord = value.match(/\\[A-Za-z]*$/)?.[0];
  return trailingControlWord ? value.slice(0, -trailingControlWord.length) : null;
}

function longestValidPrefix(value: string, displayMode: boolean, stabilizeTrailingTokens: boolean): string | null {
  let current = value;
  while (current.length > 0) {
    const validation = validate(current, displayMode);
    if (validation.ok) return current;
    if (!validation.error) return value;

    const autoClosed = autoCloseBraces(current, displayMode, validation.error);
    if (autoClosed) return autoClosed;

    if (stabilizeTrailingTokens) {
      const stripped = stripTrailingFailingToken(current);
      if (stripped && stripped !== current) {
        current = stripped;
        continue;
      }
    }

    // KaTeX reports a cursor into the string; retry with the invalid tail removed.
    const cursor = typeof validation.error.position === "number" && Number.isFinite(validation.error.position)
      ? validation.error.position
      : current.length - 1;
    const next = trimDanglingEscape(current.slice(0, Math.max(0, Math.min(cursor, current.length - 1))));
    current = next === current ? current.slice(0, -1) : next;
  }
  return null;
}

const TEXT_ESCAPES: Record<string, string> = {
  "\\": "\\textbackslash{}",
  "{": "\\{",
  "}": "\\}",
  $: "\\$",
  "#": "\\#",
  "%": "\\%",
  "&": "\\&",
  _: "\\_",
  "^": "\\textasciicircum{}",
  "~": "\\textasciitilde{}",
};

function asErroredText(prefix: string, remainder: string): string {
  const escaped = [...remainder].map((char) => TEXT_ESCAPES[char] ?? char).join("");
  const separate = /[A-Za-z0-9)\]}]/.test(prefix.at(-1) ?? "") && /[A-Za-z0-9\\]/.test(remainder[0] ?? "");
  return `\\text{${separate ? " " : ""}${escaped}}`;
}

/** Math source KaTeX can render: `"trim"` drops the unparseable tail, `"escape"` keeps it as literal text. */
export function healKatexSource(
  value: string,
  displayMode: boolean,
  errorHandling: KatexErrorHandling,
  stabilizeTrailingTokens = false,
): string | null {
  const normalized = normalizeDoubleEscapedCommands(value);
  const validation = validate(normalized, displayMode);
  if (validation.ok) return normalized;

  const autoClosed = validation.error && autoCloseBraces(normalized, displayMode, validation.error);
  if (autoClosed) return autoClosed;

  const prefix = longestValidPrefix(normalized, displayMode, stabilizeTrailingTokens);
  if (errorHandling === "trim" || prefix === normalized) return prefix;

  const validPrefix = prefix ?? "";
  const remainder = normalized.slice(validPrefix.length);
  return remainder ? `${validPrefix}${asErroredText(validPrefix, remainder)}` : prefix;
}

export function renderKatex(value: string, displayMode: boolean): string {
  return katex.renderToString(value, { displayMode, throwOnError: false });
}

function mathTarget(element: HastNode): { source: HastNode; displayMode: boolean } | null {
  if (element.tagName === "pre") {
    const code = element.children?.[0];
    return code?.type === "element" && code.tagName === "code" && classNames(code).some((name) => mathCodeLanguages.has(name))
      ? { source: code, displayMode: true }
      : null;
  }
  const classes = classNames(element);
  const display = classes.includes("math-display");
  return display || classes.includes("math-inline") || classes.some((name) => mathCodeLanguages.has(name))
    ? { source: element, displayMode: display }
    : null;
}

function replaceMath(parent: HastNode) {
  parent.children?.forEach((child, index) => {
    if (child.type !== "element") return;
    const target = mathTarget(child);
    if (!target) {
      replaceMath(child);
      return;
    }
    // Fenced math keeps its `$$` lines in the code text.
    const code = textContent(target.source).trim().replace(/^\$\$\s*([\s\S]*?)\s*\$\$$/, "$1");
    parent.children![index] = {
      type: "element",
      tagName: "katex-math",
      properties: { code, displayMode: target.displayMode ? "true" : "false" },
      children: [],
      position: child.position,
    };
  });
}

/** Replaces remark-math output with `<katex-math>` so rendering can heal per element. */
export function rehypeKatexHealing() {
  return (tree: HastNode) => replaceMath(tree);
}
