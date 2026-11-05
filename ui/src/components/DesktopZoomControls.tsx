import { useCallback, useEffect, useRef, useState } from "react";
import { Minus, Plus } from "lucide-react";
import { formatForDisplay } from "@tanstack/react-hotkeys";
import { appHotkeys } from "../hotkeys";
import { m } from "../paraglide/messages.js";
import { Button, IconButton, Tooltip } from "./ui";

export function DesktopZoomControls() {
  const [percent, setPercent] = useState(100);
  const [visible, setVisible] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const keepOpen = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
  }, []);
  const dismissLater = useCallback(() => {
    keepOpen();
    timer.current = setTimeout(() => setVisible(false), 2500);
  }, [keepOpen]);

  useEffect(() => {
    const show = (event: Event) => {
      if (!(event instanceof CustomEvent) || typeof event.detail !== "number") return;
      setPercent(event.detail);
      setVisible(true);
      if (!panel.current?.matches(":hover, :focus-within")) dismissLater();
    };
    window.addEventListener("orx:page-zoom", show);
    return () => {
      window.removeEventListener("orx:page-zoom", show);
      keepOpen();
    };
  }, [dismissLater, keepOpen]);

  const send = (action: string) => window.ipc?.postMessage(`page-zoom:${action}`);

  return (
    <div
      ref={panel}
      role="group"
      inert={!visible}
      aria-hidden={!visible}
      aria-label={m.desktop_zoom_controls()}
      className={`fixed end-4 top-14 z-300 flex items-center gap-1 rounded-full border border-border bg-background px-2 py-1 shadow-popover transition-opacity ease-out motion-reduce:transition-none ${visible ? "opacity-100 duration-120" : "pointer-events-none opacity-0 duration-150"}`}
      style={{ zoom: "calc(1 / var(--desktop-page-zoom, 1))" }}
      onMouseEnter={keepOpen}
      onMouseLeave={(event) => { if (!event.currentTarget.matches(":focus-within")) dismissLater(); }}
      onFocus={keepOpen}
      onBlur={(event) => {
        if (!(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) && !event.currentTarget.matches(":hover")) dismissLater();
      }}
    >
      <span role="status" className="min-w-12 px-1 text-sm font-medium tabular-nums text-text">{percent}%</span>
      <Tooltip content={`${m.image_zoom_out()} (${formatForDisplay(appHotkeys["zoom-out"].hotkey ?? [])})`} interactive>
        <IconButton size="small" shape="circle" aria-label={m.image_zoom_out()} disabled={percent <= 50} onClick={() => send("out")}><Minus size={14} /></IconButton>
      </Tooltip>
      <Tooltip content={`${m.image_zoom_in()} (${formatForDisplay(appHotkeys["zoom-in"].hotkey ?? [])})`} interactive>
        <IconButton size="small" shape="circle" aria-label={m.image_zoom_in()} disabled={percent >= 200} onClick={() => send("in")}><Plus size={14} /></IconButton>
      </Tooltip>
      <span aria-hidden className="mx-1 h-4 border-s border-border" />
      <Tooltip content={`${m.desktop_zoom_reset()} (${formatForDisplay(appHotkeys["zoom-reset"].hotkey ?? [])})`} interactive>
        <Button variant="ghost" size="small" disabled={percent === 100} onClick={() => send("reset")}>{m.desktop_zoom_reset()}</Button>
      </Tooltip>
    </div>
  );
}
