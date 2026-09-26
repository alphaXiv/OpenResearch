import { createContext } from "react";
import { Download } from "lucide-react";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { IconButtonLink } from "./ui";

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
      <Download size={13} />
    </IconButtonLink>
  );
}
