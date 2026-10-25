import { m } from "../paraglide/messages.js";
import { PanelLeft } from "lucide-react";
import { BrandMark } from "./Wordmark";
import { IconButton } from "./ui";

export function RailHeader({
  onCollapse,
}: {
  onCollapse?: () => void;
}) {
  return (
    <div className="rail-brand flex items-center px-3 py-4 shrink-0 mac-titlebar:ps-titlebar-controls mac-titlebar:pe-2 mac-titlebar:h-titlebar mac-titlebar:py-0">
      <div className="flex w-full items-center gap-2 px-1">
        <span className="size-7 text-primary mac-titlebar:hidden"><BrandMark /></span>
        <span className="flex-1 text-sm font-semibold tracking-tight mac-titlebar:invisible">OpenResearch</span>
        {onCollapse && (
          <IconButton size="small" data-tip={m.header_hide_sidebar()} aria-label={m.header_hide_sidebar()} onClick={onCollapse}>
            <PanelLeft size={18} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
