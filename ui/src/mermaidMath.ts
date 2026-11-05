import katex from "katex";

export type MeasuredMath = Record<string, { html: string; width: number; height: number }>;

export function measureMermaidMath(source: string): MeasuredMath {
  const result: MeasuredMath = Object.create(null);
  if (source.length > 50_000) return result;
  const expressions = [...source.matchAll(/\$\$([\s\S]*?)\$\$/g)].slice(0, 64);
  if (!expressions.length) return result;
  const probe = document.createElement("div");
  probe.style.cssText = "position:fixed;left:-10000px;top:0;visibility:hidden;display:inline-block;white-space:nowrap;font-size:16px";
  document.body.append(probe);
  try {
    for (const [, tex] of expressions) {
      if (tex.length > 2_000 || Object.hasOwn(result, tex)) continue;
      try {
        probe.innerHTML = katex.renderToString(tex, { displayMode: true, output: "mathml", throwOnError: true, trust: false, maxExpand: 100, maxSize: 20 });
        const math = probe.querySelector("math");
        if (!math) continue;
        const { width, height } = math.getBoundingClientRect();
        if (width > 0 && height > 0) result[tex] = { html: math.outerHTML, width, height };
      } catch { /* Keep invalid expressions as source. */ }
    }
  } finally {
    probe.remove();
  }
  return result;
}
