import { isExternalMarkdownTarget, isWindowsDrivePath, nativeMarkdownPath, type ImageCandidate } from "./markdownTarget";

export function resolveMermaidImages(svg: string, absoluteFileUrl: (path: string) => string, apiPrefix: string, resolve?: (src: string) => ImageCandidate[]): string | null {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  if (doc.querySelector("parsererror") || doc.documentElement.localName !== "svg") return null;
  for (const element of doc.querySelectorAll("image, a")) {
    const src = element.getAttribute("href") ?? element.getAttributeNS("http://www.w3.org/1999/xlink", "href") ?? "";
    element.removeAttribute("href");
    element.removeAttributeNS("http://www.w3.org/1999/xlink", "href");
    if (!src || src.startsWith("//") || src.startsWith("\\\\")) continue;
    if (element.localName === "a" && /^(https?:|mailto:|tel:)/i.test(src)) {
      element.setAttribute("href", src);
      element.setAttribute("target", "_blank");
      element.setAttribute("rel", "noopener noreferrer");
      continue;
    }
    if (isExternalMarkdownTarget(src) && !isWindowsDrivePath(src) && !nativeMarkdownPath(src)) continue;
    const native = nativeMarkdownPath(src);
    const url = native ? absoluteFileUrl(native) : resolve?.(src)[0]?.url;
    if (url?.startsWith(apiPrefix) && !url.includes("\\")) {
      element.setAttribute("href", url);
      if (element.localName === "a") {
        element.setAttribute("target", "_blank");
        element.setAttribute("rel", "noopener noreferrer");
      }
    }
  }
  return new XMLSerializer().serializeToString(doc.documentElement);
}
