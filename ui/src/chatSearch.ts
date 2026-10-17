export interface TextMatch {
  start: number;
  end: number;
}

/** Literal, case-insensitive matches. Unicode regexes preserve DOM offsets. */
export function findTextMatches(text: string, query: string): TextMatch[] {
  if (!query) return [];
  const pattern = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "giu");
  return Array.from(text.matchAll(pattern), (match) => ({
    start: match.index,
    end: match.index + match[0].length,
  }));
}

const BLOCK = "p,pre,li,blockquote,td,th,h1,h2,h3,h4,h5,h6,[data-message-id],.msg-user,.msg-steer";
const EXCLUDED = "button,input,textarea,select,script,style,svg,math,.sr-only,.katex-mathml,[hidden],[inert],[aria-hidden=true]";

/** Search rendered text, joining inline markup but never separate paragraphs or
 * messages. The caller mounts the full transcript while find is open. */
export function transcriptSearchRanges(root: HTMLElement, query: string): Range[] {
  if (!query) return [];
  const ranges: Range[] = [];
  let nodes: Text[] = [];
  let block: Element | null = null;
  const flush = () => {
    const text = nodes.map((node) => node.data).join("");
    for (const match of findTextMatches(text, query)) {
      const range = root.ownerDocument.createRange();
      let offset = 0;
      for (const node of nodes) {
        const end = offset + node.length;
        if (match.start >= offset && match.start < end) range.setStart(node, match.start - offset);
        if (match.end > offset && match.end <= end) {
          range.setEnd(node, match.end - offset);
          break;
        }
        offset = end;
      }
      ranges.push(range);
    }
    nodes = [];
  };
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (node instanceof Element) {
        if (node.matches(EXCLUDED)) return NodeFilter.FILTER_REJECT;
        return node.tagName === "BR" ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (node instanceof Element) {
      flush();
      continue;
    }
    const nextBlock = node.parentElement?.closest(BLOCK) ?? null;
    if (nextBlock !== block) flush();
    block = nextBlock;
    nodes.push(node as Text);
  }
  flush();
  return ranges;
}
