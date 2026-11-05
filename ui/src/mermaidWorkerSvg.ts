import { DOMParser, XMLSerializer } from "@xmldom/xmldom";

/** Prepare a large SVG off the main thread only when it needs no URL rewriting.
 * Return null for the existing browser-parser path on any unsupported input. */
export function prepareMermaidSvg(svg: string): string | null {
  try {
    const doc = new DOMParser({ onError: (_level, message) => { throw new Error(message); } }).parseFromString(svg, "image/svg+xml");
    const root = doc.documentElement;
    if (doc.doctype || !root || root.localName !== "svg" || root.namespaceURI !== "http://www.w3.org/2000/svg"
      || doc.getElementsByTagNameNS("*", "image").length || doc.getElementsByTagNameNS("*", "a").length
      || doc.getElementsByTagNameNS("*", "parsererror").length) return null;
    return new XMLSerializer().serializeToString(root);
  } catch {
    return null;
  }
}
