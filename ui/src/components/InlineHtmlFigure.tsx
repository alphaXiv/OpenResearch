import { useCallback, useEffect, useState } from "react";
import { m } from "../paraglide/messages.js";
import { htmlFigureAssetTarget, isExternalMarkdownTarget } from "../markdownTarget";
import { HtmlPreview } from "./HtmlPreview";

export function InlineHtmlFigure({ source, url, fallbackUrl, name, resolveSrc }: {
  source: string;
  url: string;
  fallbackUrl?: string | null;
  name: string;
  resolveSrc: (src: string, fallback?: boolean) => string | null;
}) {
  const [document, setDocument] = useState<{ html: string; url: string; fallback: boolean } | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setDocument(null);
    setError(false);
    async function load() {
      for (const candidate of [url, fallbackUrl]) {
        if (!candidate) continue;
        try {
          const response = await fetch(candidate, { signal: controller.signal });
          if (!response.ok || !response.headers.get("content-type")?.startsWith("text/html")) continue;
          const html = await response.text();
          if (!controller.signal.aborted) setDocument({ html, url: candidate, fallback: candidate !== url });
          return;
        } catch {
          if (controller.signal.aborted) return;
        }
      }
      if (!controller.signal.aborted) setError(true);
    }
    void load();
    return () => controller.abort();
  }, [url, fallbackUrl]);
  const resolveAsset = useCallback((src: string) => {
    if (isExternalMarkdownTarget(src)) return src;
    const target = htmlFigureAssetTarget(source, src);
    return target ? resolveSrc(target, document?.fallback) : null;
  }, [source, resolveSrc, document?.fallback]);
  return <div className="inline-html-figure my-2 w-full min-w-0">
    {document ? <HtmlPreview html={document.html} truncated={false} url={document.url} name={name} resolveSrc={resolveAsset} fitContent />
      : <div className="flex h-90 items-center justify-center text-sm text-subtext">
        {error ? <a href={url} target="_blank" rel="noopener noreferrer">{m.file_viewer_failed_to_load_file()}</a> : m.file_viewer_loading()}
      </div>}
  </div>;
}
