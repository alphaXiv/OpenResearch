import { isWindowsDrivePath } from "./nativePath.ts";

/** Normalize only parsed URL fields, leaving labels and diagram source intact. */
export function normalizeMermaidNativeTargets(model: object): void {
  const seen = new WeakSet<object>();
  const pending: { value: object; links: boolean }[] = [{ value: model, links: false }];
  const nativeUrl = (value: string) => isWindowsDrivePath(value) ? `file:///${value.replaceAll("\\", "/")}` : value;
  while (pending.length) {
    const { value, links } = pending.pop()!;
    if (seen.has(value)) continue;
    seen.add(value);
    if (value instanceof Map) {
      for (const [key, child] of value) {
        if (links && typeof child === "string") value.set(key, nativeUrl(child));
        else if (child && typeof child === "object") pending.push({ value: child, links: false });
      }
    } else {
      const fields = value as Record<string, unknown>;
      for (const key of Object.keys(fields)) {
        const child = fields[key];
        if ((key === "link" || key === "url" || key === "img") && typeof child === "string") fields[key] = nativeUrl(child);
        else if (child && typeof child === "object") pending.push({ value: child, links: key === "links" });
      }
    }
  }
}
