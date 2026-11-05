import { useQuery } from "@tanstack/react-query";
import { useWorkspace } from "../workspace";
import { m } from "../paraglide/messages.js";
import type { ImageCandidate } from "../markdownTarget";
import { HtmlPreview } from "./HtmlPreview";

export function InlineHtmlFigure({
  candidates,
  name,
}: {
  candidates: ImageCandidate[];
  name: string;
}) {
  const { workspaceKey, request } = useWorkspace();
  const document = useQuery({
    queryKey: workspaceKey(
      "inlineHtml",
      candidates.map(({ url }) => url),
    ),
    refetchOnMount: false,
    queryFn: async ({ signal }) => {
      for (let index = 0; index < candidates.length; index++) {
        try {
          const response = await request(candidates[index].url, { signal });
          if (!response.ok || !response.headers.get("content-type")?.startsWith("text/html")) continue;
          const html = await response.text();
          return { index, html };
        } catch (error) {
          if (signal.aborted) throw error;
        }
      }
      throw new Error(m.file_viewer_failed_to_load_file());
    },
  });
  const candidate = document.data && candidates[document.data.index];
  return (
    <div className="inline-html-figure my-2 w-full min-w-0">
      {document.data && candidate ? (
        <HtmlPreview
          html={document.data.html}
          truncated={false}
          url={candidate.url}
          name={name}
          resolveSrc={candidate.resolveAsset}
          fitContent
        />
      ) : (
        <div className="flex h-90 items-center justify-center text-sm text-subtext">
          {document.isError ? (
            <a
              href={candidates[0]?.url}
              target="_blank"
              rel="noopener noreferrer"
            >
              {m.file_viewer_failed_to_load_file()}
            </a>
          ) : (
            m.file_viewer_loading()
          )}
        </div>
      )}
    </div>
  );
}
