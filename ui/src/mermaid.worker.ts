import { renderAsync } from "pele/lazy";
import type { MeasuredMath } from "./mermaidMath";
import { MERMAID_SOURCE_LIMIT } from "./mermaidLimits";

self.onmessage = (event: MessageEvent<{ code: string; id: string; math: MeasuredMath }>) => {
  const { code, id, math } = event.data;
  void renderAsync(code, { limit: MERMAID_SOURCE_LIMIT, idPrefix: `orx-${id}-`, images: true, imageSchemes: ["file", ..."abcdefghijklmnopqrstuvwxyz"], links: true, linkSchemes: ["http", "https", "mailto", "tel", "file", ..."abcdefghijklmnopqrstuvwxyz"], math: (tex, size) => {
    const measured = Object.hasOwn(math, tex) ? math[tex] : undefined;
    return measured ? { html: measured.html, width: measured.width * size / 16, height: measured.height * size / 16 } : null;
  } })
    .then(async ({ svg }) => {
      if (svg.length >= 200_000) {
        try {
          const { prepareMermaidSvg } = await import("./mermaidWorkerSvg");
          const prepared = prepareMermaidSvg(svg);
          if (prepared !== null) {
            self.postMessage({ svg: prepared, prepared: true });
            return;
          }
        } catch { /* Keep the existing browser-parser path if the module cannot load. */ }
      }
      self.postMessage({ svg, prepared: false });
    })
    .catch(() => self.postMessage(null));
};
