import { renderAsync } from "pele/lazy";
import type { MeasuredMath } from "./mermaidMath";

self.onmessage = (event: MessageEvent<{ code: string; id: string; math: MeasuredMath }>) => {
  const { code, id, math } = event.data;
  void renderAsync(code, { idPrefix: `orx-${id}-`, images: true, imageSchemes: ["file", ..."abcdefghijklmnopqrstuvwxyz"], links: true, linkSchemes: ["http", "https", "mailto", "tel", "file", ..."abcdefghijklmnopqrstuvwxyz"], math: (tex, size) => {
    const measured = Object.hasOwn(math, tex) ? math[tex] : undefined;
    return measured ? { html: measured.html, width: measured.width * size / 16, height: measured.height * size / 16 } : null;
  } })
    .then(({ svg }) => self.postMessage(svg))
    .catch(() => self.postMessage(null));
};
