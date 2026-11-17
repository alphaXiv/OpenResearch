import { ImageOff } from "lucide-react";
import { useRef, useState } from "react";
import { ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import type { MediaGallery, MediaItem } from "../../../runMonitor/types";
import { useWorkspace } from "../../../workspace";
import { backendName } from "../../BackendLogos";
import { ImageModal } from "../../ImageModal";
import { Button } from "../../ui";

function Thumbnail({ item, url }: { item: MediaItem; url: string }) {
  const trigger = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [failedUrl, setFailedUrl] = useState<string>();
  const name = item.path.slice(item.path.lastIndexOf("/") + 1);
  const missing = () => {
    setFailedUrl(url);
    setExpanded(false);
  };
  if (failedUrl === url) {
    return (
      <span role="status" className="flex aspect-square flex-col items-center justify-center gap-1 rounded-sm bg-panel p-1 text-center text-xs text-ink-subtext">
        <ImageOff size={14} aria-hidden="true" />
        <span className="line-clamp-2 break-all">{m.run_media_missing({ name: ltr(name) })}</span>
      </span>
    );
  }

  return (
    <>
      <Button
        ref={trigger}
        type="button"
        aria-label={m.run_media_open({ name: ltr(name) })}
        aria-haspopup="dialog"
        className="aspect-square h-auto w-full overflow-hidden rounded-sm bg-panel p-0"
        onClick={() => setExpanded(true)}
      >
        <img src={url} alt={name} loading="lazy" className="size-full object-cover" onError={missing} />
      </Button>
      {expanded && <ImageModal key={url} src={url} alt={name} name={name} onError={missing} onClose={() => {
        setExpanded(false);
        requestAnimationFrame(() => trigger.current?.focus());
      }} />}
    </>
  );
}

export function Media({ gallery, glob, last, runId, live, page }: {
  gallery: MediaGallery | undefined; glob: string; last: number; runId: string; live: boolean; page: boolean;
}) {
  const { runImageUrl } = useWorkspace().api;
  const items = gallery?.items.slice(0, page ? last : Math.min(last, 3)) ?? [];
  const pattern = ltr(glob);
  const error = gallery?.error;
  return (
    <>
      {items.length > 0 && (
        <ul className="m-0 grid list-none grid-cols-[repeat(auto-fill,minmax(5.5rem,1fr))] gap-1.5 p-0">
          {items.map((item) => {
            const url = runImageUrl(runId, item.path, item.version);
            return <li key={item.path} className="min-w-0 max-w-32"><Thumbnail item={item} url={url} /></li>;
          })}
        </ul>
      )}
      <div role="status" className="space-y-1 text-xs text-ink-subtext">
        {error?.kind === "unsupported" && <p className="m-0">{m.run_media_unsupported({ backend: backendName(error.backend) })}</p>}
        {error?.kind === "unreadable" && <p className="m-0">{m.run_media_unreadable({ glob: pattern })}</p>}
        {!error && items.length === 0 && !gallery?.skipped && !gallery?.truncated && (
          <p className="m-0">{live ? m.run_media_none({ glob: pattern }) : m.run_media_none_ended({ glob: pattern })}</p>
        )}
        {!!gallery?.skipped && <p className="m-0">{m.run_media_skipped()}</p>}
        {gallery?.truncated && <p className="m-0">{m.run_media_truncated()}</p>}
      </div>
    </>
  );
}
