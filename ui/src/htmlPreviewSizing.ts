/** Viewport-relative pages must not repeatedly enlarge their own frame. */
export function inlineHtmlHeight(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? Math.min(1200, Math.ceil(value)) : null;
}
