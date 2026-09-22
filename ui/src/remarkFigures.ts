import { unified } from "unified";
import remarkParse from "remark-parse";

interface FigureNode {
  type: string;
  title?: string | null;
  alt?: string | null;
  url?: string;
  children?: FigureNode[];
  data?: { hName?: string; hProperties?: Record<string, unknown> };
  value?: string;
}

const captionParser = unified().use(remarkParse);

function figureKey(text: string): string | undefined {
  const match = text.trim().match(/^(?:original\s+)?(figure|fig\.?|table)\s+(\d+(?:\.\d+)*[a-z]?)(?=$|[\s:.,])/i);
  return match ? `${match[1].toLowerCase().startsWith("fig") ? "figure" : "table"} ${match[2].toLowerCase()}` : undefined;
}

function nodeText(node: FigureNode): string {
  return node.value ?? node.children?.map(nodeText).join("") ?? "";
}

export function remarkFigures() {
  return function transform(tree: FigureNode) {
    const images = new Map<string, string | null>();
    function collect(node: FigureNode) {
      if (node.type === "image" && node.url) {
        const key = figureKey(node.title ?? "") ?? figureKey(node.alt ?? "");
        if (key) images.set(key, images.has(key) && images.get(key) !== node.url ? null : node.url);
      }
      node.children?.forEach(collect);
    }
    collect(tree);
    function visit(parent: FigureNode) {
      for (const node of parent.children ?? []) {
        const key = node.type === "link" ? figureKey(nodeText(node)) : undefined;
        const src = key ? images.get(key) : undefined;
        if (src) node.data = { ...node.data, hProperties: { ...node.data?.hProperties, "data-figure-src": src } };
        const image = node.type === "paragraph" && node.children?.length === 1
          ? node.children[0] : undefined;
        if (image?.type === "image" && image.title?.trim()) {
          const caption = captionParser.parse(image.title);
          const paragraph = caption.children.length === 1 && caption.children[0]?.type === "paragraph"
            ? caption.children[0] : undefined;
          node.type = "paperFigure";
          node.data = { hName: "figure" };
          node.children = [image, {
            type: "figureCaption",
            data: { hName: "figcaption" },
            children: paragraph?.children ?? [{ type: "text", value: image.title }],
          }];
          image.title = null;
        }
        visit(node);
      }
    }
    visit(tree);
  };
}
