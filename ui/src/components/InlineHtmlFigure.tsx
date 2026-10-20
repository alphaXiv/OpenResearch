import { useEffect, useState } from "react";
import { m } from "../paraglide/messages.js";
import type { ImageCandidate } from "../markdownTarget";
import { HtmlPreview } from "./HtmlPreview";

export function InlineHtmlFigure({ candidates, name }: { candidates: ImageCandidate[]; name: string }) {
  const [document, setDocument] = useState<ImageCandidate & { html: string } | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setDocument(null);
    setError(false);
    async function load() {
      for (const candidate of candidates) {
        try {
          const response = await fetch(candidate.url, { signal: controller.signal });
          if (!response.ok || !response.headers.get("content-type")?.startsWith("text/html")) continue;
          const html = await response.text();
          if (!controller.signal.aborted) setDocument({ ...candidate, html });
          return;
        } catch {
          if (controller.signal.aborted) return;
        }
      }
      if (!controller.signal.aborted) setError(true);
    }
    void load();
    return () => controller.abort();
  }, [candidates]);
  return <div className="inline-html-figure my-2 w-full min-w-0">
    {document ? <HtmlPreview html={document.html} truncated={false} url={document.url} name={name} resolveSrc={document.resolveAsset} fitContent />
      : <div className="flex h-90 items-center justify-center text-sm text-subtext">
        {error ? <a href={candidates[0]?.url} target="_blank" rel="noopener noreferrer">{m.file_viewer_failed_to_load_file()}</a> : m.file_viewer_loading()}
      </div>}
  </div>;
}
