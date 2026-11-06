import { detectType, load, parse, render } from "pele/lazy";
import type { MeasuredMath } from "./mermaidMath";
import { MERMAID_SOURCE_LIMIT } from "./mermaidLimits";
import { normalizeMermaidNativeTargets } from "./mermaidNativeTargets";

self.onmessage = (event: MessageEvent<{ code: string; id: string; math: MeasuredMath }>) => {
  const { code, id, math } = event.data;
  const options = { limit: MERMAID_SOURCE_LIMIT, idPrefix: `orx-${id}-`, images: true, imageSchemes: ["file"], links: true, linkSchemes: ["http", "https", "mailto", "tel", "file"], math: (tex: string, size: number) => {
    const measured = Object.hasOwn(math, tex) ? math[tex] : undefined;
    return measured ? { html: measured.html, width: measured.width * size / 16, height: measured.height * size / 16 } : null;
  } };
  void (async () => {
    let type = null;
    try { type = detectType(code); } catch { /* render will report an invalid diagram */ }
    await load(type);
    const model = parse(code, options);
    normalizeMermaidNativeTargets(model);
    return render(model, options);
  })()
    .then(({ svg }) => self.postMessage(svg))
    .catch(() => self.postMessage(null));
};
