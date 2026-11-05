import { chatImageTarget, isExternalMarkdownTarget, isWindowsDrivePath, nativeMarkdownPath, type ImageCandidate } from "./markdownTarget";

export function resolveMermaidImages(svg: string, absoluteFileUrl: (path: string) => string, apiPrefix: string, resolve?: (src: string) => ImageCandidate[]): string | null {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  if (doc.querySelector("parsererror") || doc.documentElement.localName !== "svg") return null;
  for (const element of doc.querySelectorAll("image, a")) {
    const src = element.getAttribute("href") ?? element.getAttributeNS("http://www.w3.org/1999/xlink", "href") ?? "";
    element.removeAttribute("href");
    element.removeAttributeNS("http://www.w3.org/1999/xlink", "href");
    element.removeAttribute("data-orx-fallbacks");
    if (!src || src.startsWith("//") || src.startsWith("\\\\")) continue;
    if (element.localName === "a" && /^(https?:|mailto:|tel:)/i.test(src)) {
      element.setAttribute("href", src);
      element.setAttribute("target", "_blank");
      element.setAttribute("rel", "noopener noreferrer");
      continue;
    }
    if (isExternalMarkdownTarget(src) && !isWindowsDrivePath(src) && !nativeMarkdownPath(src)) continue;
    const native = nativeMarkdownPath(src) ? chatImageTarget(src) : null;
    const candidates = native?.source === "absolute"
      ? [absoluteFileUrl(native.path) + native.hash]
      : (resolve?.(src) ?? []).map(candidate => candidate.url);
    const urls = candidates.filter(url => url.startsWith(apiPrefix) && !url.includes("\\"));
    if (urls.length) {
      element.setAttribute("href", urls[0]);
      if (element.localName === "image" && urls.length > 1) element.setAttribute("data-orx-fallbacks", JSON.stringify(urls.slice(1)));
      if (element.localName === "a") {
        element.setAttribute("target", "_blank");
        element.setAttribute("rel", "noopener noreferrer");
      }
    }
  }
  return new XMLSerializer().serializeToString(doc.documentElement);
}

/** Retry only the scoped candidates that were serialized for this image. */
export function advanceMermaidImage(image: Element, apiPrefix: string): void {
  try {
    const pending: unknown = JSON.parse(image.getAttribute("data-orx-fallbacks") ?? "[]");
    if (!Array.isArray(pending)) return;
    const urls = pending.filter((url): url is string => typeof url === "string" && url.startsWith(apiPrefix) && !url.includes("\\"));
    const next = urls.shift();
    if (next) { image.setAttribute("data-orx-fallbacks", JSON.stringify(urls)); image.setAttribute("href", next); }
  } catch { /* Ignore invalid fallback metadata. */ }
}
