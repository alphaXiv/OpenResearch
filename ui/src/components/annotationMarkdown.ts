export function escapeMarkdownText(text: string): string {
  return text
    .replace(/([\\`*_[\]<>$~])/g, "\\$1")
    .replace(/(^|\n)(\s*)(#{1,6}|>|[-+]|\d+\.)\s/g, "$1$2\\$3 ")
    .replace(/(^|\n)(\s*)(=+|-{1,2})(?=\s*(?:\n|$))/g, "$1$2\\$3")
    .replace(/(^|\n)(\s*)(-{3,})(?=\s*(?:\n|$))/g, "$1$2\\$3");
}

export function inlineCodeMarkdown(text: string): string {
  const longest = Math.max(0, ...Array.from(text.matchAll(/`+/g), (match) => match[0].length));
  const delimiter = "`".repeat(longest + 1);
  const padded = /^[\s`]|[\s`]$/.test(text) ? ` ${text} ` : text;
  return `${delimiter}${padded}${delimiter}`;
}

export function fencedCodeMarkdown(text: string): string {
  const longest = Math.max(0, ...Array.from(text.matchAll(/`+/g), (match) => match[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `\n\n${fence}\n${text.replace(/^\n|\n$/g, "")}\n${fence}\n\n`;
}

export function formatMath(tex: string, display: boolean): string {
  return display ? `\n\n\\[\n${tex}\n\\]\n\n` : `\\(${tex}\\)`;
}

export function orderedListMarkdown(
  items: Array<{ markdown: string; value?: number }>,
  start = 1,
): string {
  let next = start;
  return items.map((item) => {
    const value = item.value ?? next;
    next = value + 1;
    return `${value}. ${item.markdown.trim()}`;
  }).join("\n");
}

export function listItemMarkdown(marker: string, content: string): string {
  const lines = content.trim().split("\n");
  const continuation = " ".repeat(marker.length + 1);
  return [`${marker} ${lines[0] ?? ""}`, ...lines.slice(1).map((line) => line ? `${continuation}${line}` : "")].join("\n");
}

export function tableMarkdown(rows: string[][], firstRowIsHeader: boolean): string {
  if (rows.length === 0) return "";
  const width = Math.max(...rows.map((row) => row.length));
  const cells = (row: string[]) =>
    `| ${Array.from({ length: width }, (_, i) => row[i] ?? "").join(" | ")} |`;
  const header = firstRowIsHeader ? rows[0] : Array.from({ length: width }, () => "");
  const body = firstRowIsHeader ? rows.slice(1) : rows;
  return [cells(header), cells(Array.from({ length: width }, () => "---")), ...body.map(cells)].join("\n");
}

export function headingMarkdown(tagName: string, inner: string): string | undefined {
  const level = Number(tagName.slice(1));
  return Number.isInteger(level) && level >= 1 && level <= 6
    ? `${"#".repeat(level)} ${inner.trim()}`
    : undefined;
}

export function shouldRecoverLegacyMath(text: string): boolean {
  return !text.includes("\\(") && !text.includes("\\[") && !text.includes("$$");
}

export function isLegacyFingerprintMatch(candidate: string, target: string): boolean {
  const ratio = Math.min(candidate.length, target.length) / Math.max(candidate.length, target.length);
  return ratio >= 0.8 && (candidate.includes(target) || target.includes(candidate));
}
