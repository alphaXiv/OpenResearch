import { AudioLines, ImageOff } from "lucide-react";
import { useState } from "react";
import { autoDir, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import type { MediaItem } from "../../../runMonitor/types";
import { useWorkspace } from "../../../workspace";
import { cn } from "../../ui/cn";

type MediaBlock = { media: string; last?: number; columns?: number; size?: string };

const SIZES: Record<string, string> = { s: "4rem", m: "5.5rem", l: "9rem" };

const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

/** One file: an image opens at full size, video and audio play in place on a page. */
function Item({ item, url, page }: { item: MediaItem; url: string; page: boolean }) {
  const [missing, setMissing] = useState(false);
  const name = item.caption ?? fileName(item.path);
  const frame = "block aspect-square w-full overflow-hidden rounded-sm bg-panel";
  if (missing) {
    return (
      <span className={cn(frame, "flex flex-col items-center justify-center gap-1 p-1 text-center text-xs text-ink-subtext")} title={item.path}>
        <ImageOff size={14} aria-hidden="true" />
        {page && <span className="line-clamp-2 break-all">{m.run_media_missing({ name: ltr(fileName(item.path)) })}</span>}
        {!page && <span className="sr-only">{m.run_media_missing({ name: ltr(fileName(item.path)) })}</span>}
      </span>
    );
  }
  if (item.kind === "image") {
    return (
      <a href={url} target="_blank" rel="noreferrer" className={cn(frame, "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus")}>
        <img src={url} alt={name} loading="lazy" className="size-full object-cover" onError={() => setMissing(true)} />
      </a>
    );
  }
  if (item.kind === "video") {
    return <video src={url} aria-label={name} controls={page} muted={!page} preload="metadata" className={cn(frame, "object-cover")} onError={() => setMissing(true)} />;
  }
  if (!page) return <span className={cn(frame, "flex items-center justify-center text-ink-subtext")} title={name}><AudioLines size={16} role="img" aria-label={name} /></span>;
  return <audio src={url} aria-label={name} controls preload="metadata" className="w-full" onError={() => setMissing(true)} />;
}

/** The newest files a glob under the run's artifacts matches. A strip shows three; a page the view's `last`. */
export function Media({ items, block, projectId, size, live }: { items: MediaItem[]; block: MediaBlock; projectId: string; size: "strip" | "page"; live: boolean }) {
  const { artifactUrl } = useWorkspace().api;
  const page = size === "page";
  if (items.length === 0) {
    const glob = ltr(block.media);
    return <p className="m-0 text-xs text-ink-subtext">{live ? m.run_media_none({ glob }) : m.run_media_none_ended({ glob })}</p>;
  }
  const shown = items.slice(-(page ? block.last ?? 6 : 3)).reverse();
  const track = page
    ? block.columns ? `repeat(${block.columns}, minmax(0, 1fr))` : `repeat(auto-fill, minmax(${SIZES[block.size ?? "m"] ?? SIZES.m}, 1fr))`
    : "repeat(3, 4rem)";
  return (
    <ul className="m-0 grid list-none gap-1.5 p-0" style={{ gridTemplateColumns: track }}>
      {shown.map((item) => (
        <li key={`${item.path}@${item.at}`} className={cn("min-w-0", item.kind === "audio" && page && "col-span-full")}>
          <Item item={item} url={`${artifactUrl(projectId, item.path)}&v=${item.at}`} page={page} />
          {page && <span className="mt-1 block truncate text-xs text-ink-subtext">{autoDir(item.caption ?? fileName(item.path))}</span>}
        </li>
      ))}
    </ul>
  );
}
