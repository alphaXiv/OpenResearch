import { m } from "../paraglide/messages.js";
import { useEffect, useState } from "react";
import type { FilePresentation } from "../api";

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
      <a href={url} download={name}>{m.media_preview_download()} {name}</a>
    </div>
  );
}

export function MediaPreview({
  kind,
  url,
  name,
  downloadBar = true,
}: {
  kind: MediaPreviewKind;
  url: string;
  name: string;
  /** The footer download strip. Off where the surrounding view offers its own
   * download control and the preview should fill the pane. */
  downloadBar?: boolean;
}) {
  const [failed, setFailed] = useState(false);

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
      <object
        className="fpreview-pdf block min-h-0 flex-1 w-full border-0"
        aria-label={name}
        data={url}
        type="application/pdf"
        onError={() => setFailed(true)}
      >
        <DownloadFallback url={url} name={name} />
      </object>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {preview}
      {downloadBar && (
        <div className="shrink-0 border-t border-border-variant py-1.5 px-3 text-end text-xs">
          <a href={url} download={name}>{m.media_preview_download()} {name}</a>
        </div>
      )}
    </div>
  );
}
