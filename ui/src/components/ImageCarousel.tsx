import { Children, createContext, isValidElement, useRef, useState, type Dispatch, type SetStateAction, type ReactNode } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { m } from "../paraglide/messages.js";
import { IconButton } from "./ui";

export const ImageCarouselContext = createContext<{
  zoom: number;
  setZoom: Dispatch<SetStateAction<number>>;
  expanded: boolean;
  setExpanded: (expanded: boolean) => void;
} | null>(null);

export function ImageCarousel({ children }: { children: ReactNode }) {
  const images = Children.toArray(children).filter(isValidElement);
  const [selected, setSelected] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const [zoom, setZoom] = useState(1);
  const region = useRef<HTMLDivElement>(null);
  const index = Math.min(selected, images.length - 1);
  return <ImageCarouselContext value={{ zoom, setZoom, expanded, setExpanded: (open) => {
    setExpanded(open);
    if (!open) requestAnimationFrame(() => region.current?.focus());
  } }}>
    <div ref={region} className="mt-6 mb-3 outline-none" role="region" tabIndex={0} aria-label={m.media_preview_images()}
    onKeyDown={(event) => {
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        event.stopPropagation();
        setSelected(Math.max(0, Math.min(images.length - 1, index + (event.key === "ArrowRight" ? 1 : -1))));
        if (!expanded) event.currentTarget.focus();
      }
    }}>
    <div className="flex h-100 items-center justify-center overflow-auto [&>p]:m-0 [&>figure]:m-0 [&_img]:max-h-100 [&_img]:mx-auto [&_img]:my-0 [&_img]:object-contain">{images[index]}</div>
    <div className="flex items-center justify-center gap-3 mt-6">
      <IconButton size="small" disabled={index === 0} aria-label={m.media_preview_previous_image()}
        onClick={() => setSelected(index - 1)}><ChevronLeft size={16} /></IconButton>
      <span className="text-sm text-subtext tabular-nums" aria-live="polite">{index + 1} / {images.length}</span>
      <IconButton size="small" disabled={index === images.length - 1} aria-label={m.media_preview_next_image()}
        onClick={() => setSelected(index + 1)}><ChevronRight size={16} /></IconButton>
    </div>
    </div>
  </ImageCarouselContext>;
}
