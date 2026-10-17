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

const BLOCK = "p,pre,li,blockquote,td,th,h1,h2,h3,h4,h5,h6,details,summary,[data-message-id],.msg-user,.msg-steer";
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
      // Only the first direct summary is displayed while a details is closed.
      // Reject text nodes too: disclosure bodies need not have an element wrapper.
      const parent = node.parentElement;
      if (parent?.matches("details:not([open])") && node !== parent.querySelector(":scope > summary")) {
        return NodeFilter.FILTER_REJECT;
      }
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

export function scrollTranscriptSearchRangeIntoView(range: Range, scroll: HTMLElement): void {
  // Reveal the match from the innermost output/code viewport outwards. Read its
  // position again after each scroll, then position the now-visible match in chat.
  for (let parent = range.startContainer.parentElement; parent && parent !== scroll; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    const match = range.getBoundingClientRect();
    const bounds = parent.getBoundingClientRect();
    if (parent.scrollHeight > parent.clientHeight && /(auto|scroll)/.test(style.overflowY)) {
      parent.scrollTop += match.top - bounds.top - parent.clientTop - (parent.clientHeight - match.height) / 2;
    }
    if (parent.scrollWidth > parent.clientWidth && /(auto|scroll)/.test(style.overflowX)) {
      parent.scrollLeft += match.left - bounds.left - parent.clientLeft - (parent.clientWidth - match.width) / 2;
    }
  }
  const rect = range.getBoundingClientRect();
  const viewport = scroll.getBoundingClientRect();
  scroll.scrollTop += rect.top - viewport.top - scroll.clientTop - (scroll.clientHeight - rect.height) / 2;
}
