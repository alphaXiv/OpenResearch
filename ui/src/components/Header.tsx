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
    <div className="rail-brand flex h-12 items-center px-3 border-b border-border shrink-0 mac-titlebar:ps-20 mac-titlebar:pe-2 mac-titlebar:h-11 mac-titlebar:border-b-0">
      <div className="flex w-full items-center gap-2 px-1">
        <span className="w-5 h-5 mac-titlebar:hidden"><BrandMark /></span>
        <span className="flex-1 text-base font-medium mac-titlebar:invisible">OpenResearch</span>
        {onCollapse && (
          <IconButton size="small" data-tip={m.header_hide_sidebar()} aria-label={m.header_hide_sidebar()} onClick={onCollapse}>
            <PanelLeft size={18} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
