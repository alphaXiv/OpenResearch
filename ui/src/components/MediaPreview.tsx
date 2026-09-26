import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { lazy, Suspense, useContext, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import type { FilePresentation } from "../api";
import { MediaDownloadButton, MediaToolbarSlot } from "./mediaToolbar";
import { Spinner } from "./ui";

// PDF.js is large, so it loads with the first PDF rather than the dashboard. A
// tab left open across an update asks for a chunk the new build doesn't have.
const PdfPreview = lazy(() =>
  import("./PdfPreview").catch((error: unknown) => {
    console.error("PDF viewer failed to load", error);
    return { default: DownloadFallback };
  }),
);

export type MediaPreviewKind = Exclude<FilePresentation, "text" | "unknown" | "download">;

export function mediaPreviewKind(
  presentation: FilePresentation | undefined,
): MediaPreviewKind | null {
  if (
    presentation === "image" ||
    presentation === "audio" ||
    presentation === "video" ||
    presentation === "pdf"
  ) {
    return presentation;
  }
  return null;
}

function DownloadFallback({ url, name }: { url: string; name: string }) {
  return (
    <div className="file-view-note py-2.5 px-4 text-sm text-muted">
      {m.media_preview_this_browser_can_t_preview_this_media_format()}{" "}
      <a href={url} download={name}>{m.media_preview_download()} {ltr(name)}</a>
    </div>
  );
}

export function MediaPreview({
  kind,
  url,
  name,
  download = true,
}: {
  kind: MediaPreviewKind;
  url: string;
  name: string;
  /** Off where the surrounding view already offers its own download control. */
  download?: boolean;
}) {
  const [failed, setFailed] = useState(false);
  const toolbarSlot = useContext(MediaToolbarSlot);

  useEffect(() => setFailed(false), [kind, url]);

  if (failed) return <DownloadFallback url={url} name={name} />;

  let preview;
  if (kind === "image") {
    preview = (
      <div className="fpreview-image flex min-h-0 flex-1 items-start justify-center overflow-auto p-6 [&_img]:max-w-full [&_img]:h-auto [&_img]:border [&_img]:border-border [&_img]:rounded-sm">
        <img src={url} alt={name} onError={() => setFailed(true)} />
      </div>
    );
  } else if (kind === "audio") {
    preview = (
      <div className="flex min-h-0 flex-1 items-center justify-center p-6">
        <audio
          className="w-full max-w-160"
          controls
          preload="metadata"
          src={url}
          aria-label={name}
          onError={() => setFailed(true)}
       />
      </div>
    );
  } else if (kind === "video") {
    preview = (
      <div className="flex min-h-0 flex-1 items-center justify-center p-6">
        <video
          className="max-h-full max-w-full rounded-sm border border-border"
          controls
          preload="metadata"
          src={url}
          aria-label={name}
          onError={() => setFailed(true)}
       />
      </div>
    );
  } else {
    preview = (
      <Suspense
        fallback={
          <div className="flex min-h-0 flex-1 items-center justify-center">
            <Spinner />
          </div>
        }
      >
        <PdfPreview key={url} url={url} name={name} download={download} onError={() => setFailed(true)} />
      </Suspense>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {preview}
      {/* PdfPreview renders its own so the button stays after its lazily mounted controls. */}
      {download && kind !== "pdf" && toolbarSlot &&
        createPortal(<MediaDownloadButton url={url} name={name} />, toolbarSlot)}
    </div>
  );
}
