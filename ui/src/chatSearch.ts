import { unified } from "unified";
import remarkParse from "remark-parse";

type SnippetNode = { type: string; value?: string; alt?: string | null; children?: SnippetNode[] };

const snippetParser = unified().use(remarkParse);

export function chatSearchSnippet(text: string): string {
  const visit = (node: SnippetNode): string => {
    if (node.type === "text" || node.type === "code" || node.type === "inlineCode") return node.value ?? "";
    if (node.type === "image") return node.alt ?? "";
    if (node.type === "break") return " ";
    if (node.children) return node.children.map(visit).join(["root", "list", "listItem", "blockquote"].includes(node.type) ? " " : "");
    return "";
  };
  return visit(snippetParser.parse(text)).replace(/\s+/gu, " ").trim();
}

export function chatSearchRanges(root: HTMLElement, query: string): Range[] {
  const terms = [...new Set(query.toLowerCase().split(/\s+/u).filter(Boolean))];
  if (!terms.length) return [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => node.parentElement?.closest(".sr-only, [inert], [aria-hidden=true], script, style") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const nodes: { node: Node; start: number; end: number }[] = [];
  let text = "";
  while (walker.nextNode()) {
    const node = walker.currentNode;
    const value = node.textContent ?? "";
    nodes.push({ node, start: text.length, end: text.length + value.length });
    text += value;
  }
  let folded = "";
  let offset = 0;
  const starts: number[] = [], ends: number[] = [];
  for (const character of text) {
    const lower = character.toLowerCase();
    for (let index = 0; index < lower.length; index++) {
      starts.push(offset);
      ends.push(offset + character.length);
    }
    folded += lower;
    offset += character.length;
  }
  const matches: [number, number][] = [];
  let anchor: [number, number] | undefined;
  let fewest = Infinity;
  for (const term of terms) {
    const first = matches.length;
    let from = 0;
    while (matches.length - first < 1000) {
      const at = folded.indexOf(term, from);
      if (at < 0) break;
      matches.push([starts[at], ends[at + term.length - 1]]);
      from = at + term.length;
    }
    const count = matches.length - first;
    if (count && count < fewest) { fewest = count; anchor = matches[first]; }
  }
  matches.sort((a, b) => a[0] - b[0]);
  const ranges: Range[] = [];
  let startIndex = 0;
  for (const [start, end] of matches) {
    const primary = start === anchor?.[0] && end === anchor[1];
    if (!primary && ranges.length >= 999) continue;
    while (startIndex < nodes.length && nodes[startIndex].end <= start) startIndex++;
    let endIndex = startIndex;
    while (endIndex < nodes.length && nodes[endIndex].end < end) endIndex++;
    if (!nodes[startIndex] || !nodes[endIndex]) continue;
    const range = document.createRange();
    range.setStart(nodes[startIndex].node, start - nodes[startIndex].start);
    range.setEnd(nodes[endIndex].node, end - nodes[endIndex].start);
    if (range.getClientRects().length) {
      if (primary) ranges.unshift(range);
      else ranges.push(range);
    }
  }
  return ranges;
}
