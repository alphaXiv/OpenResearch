import { fmtDuration } from "../api";
import { autoDir, ltr } from "../i18n";
import { getLocale } from "../paraglide/runtime.js";
import { m } from "../paraglide/messages.js";
import type { Bindings, SignalSpec, SignalValue, Signals, Versus } from "./types";

const number = (digits: number | undefined, options: Intl.NumberFormatOptions = {}) =>
  new Intl.NumberFormat(getLocale(), digits === undefined ? options : { minimumFractionDigits: digits, maximumFractionDigits: digits, ...options });

/**
 * A number no view gave digits, by its size: to three significant figures and never fewer than its whole part, and one
 * too small or too large for that in scientific notation, as code writes it ("3e-4"). A nonzero value never reads as
 * zero, and negative zero reads as zero. `orx exp status` prints the same characters.
 */
export function fmtMeasure(value: number, options: Intl.NumberFormatOptions = {}): string {
  const size = Math.abs(value);
  if (size !== 0 && (size < 1e-3 || size >= 1e15)) {
    return new Intl.NumberFormat("en", { notation: "scientific", maximumSignificantDigits: 3, ...options }).format(value).replace("E", "e");
  }
  const whole = size === 0 ? 0 : Number(size.toExponential().split("e")[1]) + 1;
  const unsigned = value === 0 ? 0 : value;
  return new Intl.NumberFormat(getLocale(), { maximumSignificantDigits: Math.max(3, whole), ...options }).format(unsigned);
}

/** The word a view gives a raw value, such as "valid" for 1. */
export const labelOf = (spec: SignalSpec | undefined, value: SignalValue | undefined) =>
  value === null || value === undefined ? undefined : spec?.labels?.[String(value)];

/** One measurement the way its view declared it; a value that isn't a number is shown as the server sent it. */
export function fmtSignal(spec: SignalSpec | undefined, value: SignalValue | undefined): string {
  if (value === null || value === undefined) return "—";
  const word = labelOf(spec, value);
  if (word !== undefined) return word;
  if (typeof value === "string") return value === "Infinity" ? "∞" : value === "-Infinity" ? "-∞" : value;
  if (spec?.as === "duration") return fmtDuration(value * 1000);
  if (spec?.as === "compact") return number(undefined, { notation: "compact" }).format(value);
  const scaled = value * (spec?.scale ?? 1);
  const options: Intl.NumberFormatOptions = {
    ...(spec?.sign ? { signDisplay: "exceptZero" } : {}),
    ...(spec?.unit === "%" ? { style: "unit", unit: "percent" } : {}),
  };
  if (spec?.digits === undefined) return fmtMeasure(scaled, options);
  return number(Math.abs(scaled) >= 1000 ? 0 : spec.digits, options).format(scaled);
}

/** A difference from the parent, rounded to the signal's digits. */
export const fmtMagnitude = (spec: SignalSpec | undefined, magnitude: number) =>
  spec?.digits === undefined ? fmtMeasure(magnitude) : number(spec.digits).format(magnitude);

type Comparison = Pick<Versus, "d" | "good" | "flat" | "mag">;

export function versusOf(spec: SignalSpec | undefined, d: number): Comparison | null {
  if (!spec?.better) return null;
  const size = Math.abs(d * (spec.scale ?? 1));
  const unit = 10 ** (spec.digits ?? 0);
  const mag = spec.digits === undefined ? size : Math.round(size * unit) / unit;
  return { d, good: spec.better === "lower" ? d < 0 : d > 0, flat: mag === 0, mag };
}

/** Each `{token}` in a watcher's sentence: `{signal}`, `{signal.vs}` or `{count:"pattern"}`. */
const TOKEN = /\{([^{}]+)\}/g;

/** The signals a sentence names, so the line need not repeat them. */
export const tokensOf = (template: string) => new Set([...template.matchAll(TOKEN)].map((match) => match[1]));

/** A watcher's sentence with each token filled from the live values; `**` marks stay for the renderer. */
export function fillSentence(template: string, bindings: Bindings, signals: Signals): string {
  const lineCount = /^\*\*\{(count:"(?:[^"\\]|\\.)*")\}\*\* lines (?:of output|like "([\s\S]*)") so far\.$/u.exec(template);
  if (lineCount) {
    const [, key, pattern] = lineCount;
    const value = bindings[key];
    if (typeof value === "number") {
      const one = new Intl.PluralRules(getLocale()).select(value) === "one";
      const count = `**${ltr(number(0).format(value))}**`;
      if (pattern === undefined) return (one ? m.run_output_lines_one : m.run_output_lines_other)({ count });
      return (one ? m.run_output_matching_one : m.run_output_matching_other)({ count, pattern: autoDir(pattern) });
    }
  }

  // A later step can lose the parent comparison the watcher had when it wrote this clause.
  const text = template.replace(/[,،，]\s*(\*\*)?\{([^{}]+\.vs)\}\1(?=[.!?…。۔؟।]?\s*$)/u, (clause, _bold, key: string) => {
    const comparison = bindings[key];
    return comparison !== null && typeof comparison === "object" ? clause : "";
  });

  const labels = [...tokensOf(text)].flatMap((key) => {
    const name = key.replace(/\.vs$/u, "");
    const label = Object.hasOwn(signals, name) ? signals[name].label ?? name : "";
    return label ? [label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")] : [];
  });
  const literalLabel = labels.length
    ? new RegExp(`(^|[^\\p{L}\\p{N}_])(${labels.sort((a, b) => b.length - a.length).join("|")})(?=$|[^\\p{L}\\p{N}_])`, "gu")
    : null;

  return text.split(TOKEN).map((part, index) => {
    if (index % 2 === 0) return literalLabel ? part.replace(literalLabel, (_, before: string, label: string) => before + autoDir(label)) : part;
    const key = part;
    if (!Object.hasOwn(bindings, key)) return "—";
    const value = bindings[key];
    if (key.startsWith("count:")) return typeof value === "number" ? ltr(number(0).format(value)) : "—";
    if (key.endsWith(".vs")) {
      const name = key.slice(0, -3);
      const vs = value;
      if (vs === null || typeof vs !== "object") return "—";
      if (vs.flat) return autoDir(m.run_vs_token_level());
      const delta = ltr(fmtMagnitude(signals[name], vs.mag));
      return autoDir(vs.good ? m.run_vs_token_better({ delta }) : m.run_vs_token_worse({ delta }));
    }
    const spec = signals[key];
    const signal = typeof value === "object" ? null : value;
    const formatted = fmtSignal(spec, signal);
    return typeof signal === "number" && labelOf(spec, signal) === undefined && spec?.as !== "duration" ? ltr(formatted) : autoDir(formatted);
  }).join("");
}

export const plainSentence = (filled: string) => filled.replaceAll("**", "");

export const fmtUsd = (value: number) => new Intl.NumberFormat(getLocale(), { style: "currency", currency: "USD" }).format(value);

export const fmtPercent = (fraction: number) => new Intl.NumberFormat(getLocale(), { style: "percent" }).format(fraction);
