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
    <div className="rail-brand px-3 py-3 border-b border-border shrink-0">
      <div className="flex items-center gap-2 px-1">
        <span className="w-5 h-5"><BrandMark /></span>
        <span className="flex-1 text-base font-medium">OpenResearch</span>
        {onCollapse && (
          <IconButton size="small" data-tip={m.header_hide_sidebar()} aria-label={m.header_hide_sidebar()} onClick={onCollapse}>
            <PanelLeft size={18} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
