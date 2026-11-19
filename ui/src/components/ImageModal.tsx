import { Download, Minus, Plus, X } from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";
import { imageWheelZoom } from "../imageZoom";
import { m } from "../paraglide/messages.js";
import { ImageCarouselContext } from "./ImageCarousel";
import { Button, Dialog, DialogContent, IconButton, IconButtonLink, Tooltip } from "./ui";

export function ImageModal({ src, alt, name, onClose, onError }: {
  src: string; alt: string; name?: string; onClose: () => void; onError?: () => void;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [localZoom, setLocalZoom] = useState(1);
  const carousel = useContext(ImageCarouselContext);
  const zoom = carousel?.zoom ?? localZoom;
  const setZoom = carousel?.setZoom ?? setLocalZoom;
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    const area = viewport;
    if (!area) return;
    const observer = new ResizeObserver(([entry]) => {
      setBounds({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      setZoom((value) => imageWheelZoom(value, event.deltaY, event.deltaMode, area.clientHeight));
    };
    observer.observe(area);
    area.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      area.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [viewport, setZoom]);
  const fit = size.width && bounds.width
    ? Math.min(1, bounds.width * 0.9 / size.width, bounds.height * 0.9 / size.height)
    : 1;
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
    <DialogContent ref={dialog} initialFocus={dialog} aria-label={alt || m.chat_pasted_image()}
      className="fixed inset-0 h-dvh w-screen max-w-none rounded-none border-0 bg-transparent p-0 text-text shadow-none"
      onClick={(event) => { event.stopPropagation(); if (event.target === event.currentTarget) onClose(); }}>
      <div ref={setViewport} className="absolute inset-x-4 sm:inset-x-12 top-16 bottom-32 overflow-auto overscroll-contain"
        onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
        <div className="flex min-h-full min-w-full w-max pointer-events-none">
          <img src={src} alt={alt} onError={onError} onLoad={(event) => setSize({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
            style={size.width ? { width: size.width * fit * zoom, height: size.height * fit * zoom } : undefined}
            className={`pointer-events-auto block m-auto shrink-0 max-w-none rounded-lg shadow-modal ${size.width && bounds.width ? "visible" : "invisible"}`} />
        </div>
      </div>
      <div className="absolute top-5 right-5 flex gap-2">
        <Tooltip interactive content={m.media_preview_download()}>
          <IconButtonLink shape="circle" href={src} download={name || true} aria-label={m.media_preview_download()}
            className="bg-background shadow-popover"><Download size={16} /></IconButtonLink>
        </Tooltip>
        <Tooltip interactive content={m.tour_close()}>
          <IconButton shape="circle" onClick={onClose} aria-label={m.tour_close()}
            className="bg-background shadow-popover"><X size={16} /></IconButton>
        </Tooltip>
      </div>
      <div className="absolute bottom-6 inset-x-6 flex flex-col items-center gap-3 pointer-events-none">
        {alt && <div className="max-w-xl line-clamp-2 rounded-xl bg-background px-4 py-2 text-center text-sm leading-snug shadow-popover">{alt}</div>}
        <div className="pointer-events-auto flex items-center gap-1 rounded-full bg-background p-1.5 shadow-popover">
          <Tooltip interactive content={m.image_zoom_out()}>
            <IconButton shape="circle" disabled={zoom <= 0.25} onClick={() => setZoom((value) => Math.max(0.25, value - 0.25))}
              aria-label={m.image_zoom_out()}><Minus size={16} /></IconButton>
          </Tooltip>
          <Tooltip interactive content={m.image_zoom_reset()}>
            <Button variant="ghost" className="min-w-16 tabular-nums" onClick={() => setZoom(1)} aria-label={m.image_zoom_reset()}>{Math.round(zoom * 100)}%</Button>
          </Tooltip>
          <Tooltip interactive content={m.image_zoom_in()}>
            <IconButton shape="circle" disabled={zoom >= 4} onClick={() => setZoom((value) => Math.min(4, value + 0.25))}
              aria-label={m.image_zoom_in()}><Plus size={16} /></IconButton>
          </Tooltip>
        </div>
      </div>
    </DialogContent>
  </Dialog>;
}
