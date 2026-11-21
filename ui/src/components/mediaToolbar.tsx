import { createContext, type RefObject } from "react";
import { Copy, Download } from "lucide-react";
import { copyImageToClipboard } from "../clipboard";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { IconButton, IconButtonLink } from "./ui";

// A file header's element that media previews portal their controls into.
export const MediaToolbarSlot = createContext<HTMLElement | null>(null);

export function MediaDownloadButton({ url, name }: { url: string; name: string }) {
  return (
    <IconButtonLink
      size="small"
      href={url}
      download={name}
      aria-label={m.a11y_download_file({ name: ltr(name) })}
      data-tip={m.a11y_download_file({ name: ltr(name) })}
      data-tip-align="end"
    >
      <Download size={14} />
    </IconButtonLink>
  );
}

export function MediaCopyButton({ image }: { image: RefObject<HTMLImageElement | null> }) {
  return (
    <IconButton
      size="small"
      aria-label={m.image_copy()}
      data-tip={m.image_copy()}
      data-tip-align="end"
      onClick={() => { if (image.current) void copyImageToClipboard(image.current); }}
    >
      <Copy size={14} />
    </IconButton>
  );
}
