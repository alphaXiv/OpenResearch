const CURRENCY_AMOUNT = /^\d+(?:,\d{3})*(?:\.\d+)?(?:\s*[–—-]\s*\$?\d+(?:,\d{3})*(?:\.\d+)?)?(?:\/[A-Za-z][A-Za-z0-9-]*)?/;

interface NormalizationOptions {
  predictMath?: boolean;
}

function runLength(text: string, index: number, character: string): number {
  let end = index;
  while (text[end] === character) end += 1;
  return end - index;
}

function isEscaped(text: string, index: number): boolean {
  let slashes = 0;
  for (let cursor = index - 1; cursor >= 0 && text[cursor] === "\\"; cursor -= 1) slashes += 1;
  return slashes % 2 === 1;
}

interface FenceMarker {
  hasListMarker: boolean;
  indentation: number;
  listIndent: number;
  offset: number;
  quoteDepth: number;
}

function fenceMarker(line: string): FenceMarker {
  let hasListMarker = false;
  let listIndent = 0;
  let offset = 0;
  let quoteDepth = 0;
  while (offset < line.length) {
    const quote = /^ {0,3}>[ \t]?/.exec(line.slice(offset));
    if (quote) {
      offset += quote[0].length;
      quoteDepth += 1;
      continue;
    }
    const list = /^ {0,3}(?:[-+*]|\d+[.)])[ \t]+/.exec(line.slice(offset));
    if (!list) break;
    offset += list[0].length;
    listIndent += list[0].length;
    hasListMarker = true;
  }
  const indentation = /^[ \t]*/.exec(line.slice(offset))?.[0].length ?? 0;
  return {
    hasListMarker,
    indentation,
    listIndent,
    offset: offset + indentation,
    quoteDepth,
  };
}

function isFenceStart(text: string, index: number): boolean {
  const character = text[index];
  if (character !== "`" && character !== "~") return false;
  if (isEscaped(text, index)) return false;
  if (runLength(text, index, character) < 3) return false;
  const lineStart = text.lastIndexOf("\n", index - 1) + 1;
  const lineEnd = text.indexOf("\n", index);
  const line = text.slice(lineStart, lineEnd === -1 ? text.length : lineEnd);
  const marker = fenceMarker(line);
  return marker.indentation <= 3 && lineStart + marker.offset === index;
}

function fencedRegionEnd(text: string, index: number): number {
  const character = text[index]!;
  const openingLength = runLength(text, index, character);
  const openingLineStart = text.lastIndexOf("\n", index - 1) + 1;
  const openingLineEnd = text.indexOf("\n", index);
  const openingMarker = fenceMarker(text.slice(openingLineStart, openingLineEnd === -1 ? text.length : openingLineEnd));
  let lineStart = text.indexOf("\n", index + openingLength);
  if (lineStart === -1) return text.length;
  lineStart += 1;

  while (lineStart < text.length) {
    const lineEnd = text.indexOf("\n", lineStart);
    const end = lineEnd === -1 ? text.length : lineEnd;
    const line = text.slice(lineStart, end);
    const candidate = fenceMarker(line);
    const marker = lineStart + candidate.offset;
    const closingLength = runLength(text, marker, character);
    const compatibleContainer = candidate.quoteDepth === openingMarker.quoteDepth
      && !candidate.hasListMarker
      && candidate.indentation >= openingMarker.listIndent
      && candidate.indentation <= openingMarker.listIndent + 3;
    if (compatibleContainer && closingLength >= openingLength && /^[ \t\r]*$/.test(text.slice(marker + closingLength, end))) {
      return lineEnd === -1 ? text.length : lineEnd + 1;
    }
    if (lineEnd === -1) return text.length;
    lineStart = lineEnd + 1;
  }
  return text.length;
}

function inlineCodeRegionEnd(text: string, index: number, allowUnclosed: boolean): number | null {
  const length = runLength(text, index, "`");
  let candidate = index + length;
  while (candidate < text.length) {
    candidate = text.indexOf("`".repeat(length), candidate);
    if (candidate === -1) return allowUnclosed ? text.length : null;
    if (text[candidate - 1] !== "`" && text[candidate + length] !== "`") {
      return candidate + length;
    }
    candidate += length;
  }
  return allowUnclosed ? text.length : null;
}

function mentionRegionEnd(text: string, index: number, allowUnclosed: boolean): number | null {
  if (!/^<(?:file|run)\b/i.test(text.slice(index))) return null;
  let quote: string | null = null;
  for (let cursor = index + 1; cursor < text.length; cursor += 1) {
    const character = text[cursor]!;
    if (quote) {
      if (character === quote && text[cursor - 1] !== "\\") quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === ">") {
      return cursor + 1;
    }
  }
  return allowUnclosed ? text.length : null;
}

function unescapedSingleDollarIndices(text: string): number[] {
  const indices: number[] = [];
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] !== "$") continue;
    if (isEscaped(text, index)) continue;
    if (text[index - 1] !== "$" && text[index + 1] !== "$") indices.push(index);
  }
  return indices;
}

function looksLikeNumberLedMath(content: string, amountLength: number): boolean {
  const remainder = content.slice(amountLength);
  if (!remainder) return true;
  if (!/^[\s\dA-Za-z.,%+*/=^_{}\\<>|()[\]-]+$/.test(remainder)) return false;
  if (/^[eE][+-]?\d+$/.test(remainder)) return true;
  if (/[+*/=^_{}\\<>|()]/.test(remainder)) return true;
  return /^[A-Za-z][A-Za-z0-9]*$/.test(remainder);
}

/** Escape legacy currency markers without mutating stored transcript content. */
export function normalizeCurrencyDollars(
  text: string,
  { predictMath = false }: NormalizationOptions = {},
): string {
  const dollars = unescapedSingleDollarIndices(text);
  const escaped = new Set<number>();
  const protectedMath = new Set<number>();

  for (let position = 0; position < dollars.length; position += 1) {
    const index = dollars[position]!;
    if (protectedMath.has(index)) continue;
    const next = dollars[position + 1];
    const amount = CURRENCY_AMOUNT.exec(text.slice(index + 1));

    if (!amount) {
      if (next != null) {
        protectedMath.add(index);
        protectedMath.add(next);
        position += 1;
      }
      continue;
    }

    if (next == null) {
      const content = text.slice(index + 1);
      const strongCurrency = /[–—]|\/[A-Za-z]/.test(amount[0]);
      const plausibleMath = content.length === amount[0].length
        || looksLikeNumberLedMath(content, amount[0].length);
      if (!predictMath || strongCurrency || !plausibleMath) escaped.add(index);
      continue;
    }

    if (CURRENCY_AMOUNT.test(text.slice(next + 1))) {
      escaped.add(index);
      continue;
    }

    if (/[A-Za-z]/.test(text[next + 1] ?? "")) {
      escaped.add(index);
      continue;
    }

    protectedMath.add(index);
    protectedMath.add(next);
    position += 1;
  }

  const mathFlowTriples = new Set<number>();
  const flowCandidates: number[] = [];
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] !== "$" || runLength(text, index, "$") !== 3 || isEscaped(text, index)) continue;
    const lineStart = text.lastIndexOf("\n", index - 1) + 1;
    const lineEndAt = text.indexOf("\n", index + 3);
    const lineEnd = lineEndAt === -1 ? text.length : lineEndAt;
    if (/^[ \t]*$/.test(text.slice(lineStart, index)) && /^[ \t\r]*$/.test(text.slice(index + 3, lineEnd))) {
      flowCandidates.push(index);
    }
  }
  for (let index = 0; index + 1 < flowCandidates.length; index += 2) {
    mathFlowTriples.add(flowCandidates[index]!);
    mathFlowTriples.add(flowCandidates[index + 1]!);
  }

  const standaloneTriples = new Set<number>();
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] !== "$") continue;
    const length = runLength(text, index, "$");
    if (length === 3 && !isEscaped(text, index) && !mathFlowTriples.has(index)) {
      standaloneTriples.add(index);
    }
    index += length - 1;
  }
  let normalized = "";
  for (let index = 0; index < text.length; index += 1) {
    if (standaloneTriples.has(index)) {
      normalized += "\\$\\$\\$";
      index += 2;
    } else if (escaped.has(index)) {
      normalized += "\\$";
    } else {
      normalized += text[index];
    }
  }
  return normalized;
}

function normalizeProse(text: string, options: NormalizationOptions): string {
  let normalizedMath = text
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, inner: string) => `$$${inner}$$`)
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, inner: string) => `$$${inner}$$`);
  if (options.predictMath) {
    normalizedMath = normalizedMath
      .replace(/\\\[([\s\S]*)$/, (_, inner: string) => `$$${inner}`)
      .replace(/\\\(([\s\S]*)$/, (_, inner: string) => `$$${inner}`);
  }
  return normalizeCurrencyDollars(normalizedMath, options);
}

/** Normalize math and legacy currency while leaving code and mention tags opaque. */
export function normalizeMarkdownForRendering(
  text: string,
  options: NormalizationOptions = {},
): string {
  let normalized = "";
  let proseStart = 0;
  let index = 0;

  while (index < text.length) {
    let opaqueEnd: number | null = null;
    if (isFenceStart(text, index)) {
      opaqueEnd = fencedRegionEnd(text, index);
    } else if (text[index] === "`" && !isEscaped(text, index)) {
      opaqueEnd = inlineCodeRegionEnd(text, index, options.predictMath === true);
    } else if (text[index] === "<") {
      opaqueEnd = mentionRegionEnd(text, index, options.predictMath === true);
    }

    if (opaqueEnd == null) {
      index += 1;
      continue;
    }

    normalized += normalizeProse(text.slice(proseStart, index), options);
    normalized += text.slice(index, opaqueEnd);
    index = opaqueEnd;
    proseStart = opaqueEnd;
  }

  normalized += normalizeProse(text.slice(proseStart), options);
  return normalized;
}
