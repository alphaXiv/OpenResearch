import { isExternalMarkdownTarget, isWindowsDrivePath, nativeMarkdownPath, type ImageCandidate } from "./markdownTarget";

export function resolveMermaidImages(svg: string, absoluteFileUrl: (path: string) => string, apiPrefix: string, resolve?: (src: string) => ImageCandidate[]): string | null {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  if (doc.querySelector("parsererror") || doc.documentElement.localName !== "svg") return null;
  for (const image of doc.querySelectorAll("image")) {
    const src = image.getAttribute("href") ?? image.getAttributeNS("http://www.w3.org/1999/xlink", "href") ?? "";
    image.removeAttribute("href");
    image.removeAttributeNS("http://www.w3.org/1999/xlink", "href");
    if (!src || src.startsWith("//") || src.startsWith("\\\\")) continue;
    if (isExternalMarkdownTarget(src) && !isWindowsDrivePath(src) && !nativeMarkdownPath(src)) continue;
    const native = nativeMarkdownPath(src);
    const url = native ? absoluteFileUrl(native) : resolve?.(src)[0]?.url;
    if (url?.startsWith(apiPrefix) && !url.includes("\\")) image.setAttribute("href", url);
  }
  return new XMLSerializer().serializeToString(doc.documentElement);
}
