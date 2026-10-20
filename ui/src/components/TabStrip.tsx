import { useCallback, useEffect, useRef, useState } from "react";

/** Horizontally scrolling tab row with an overlay scroll thumb on the tab bar's bottom border. */
export function TabStrip({ activeKey, children }: { activeKey: string; children: React.ReactNode }) {
  const stripRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  const [thumbVisible, setThumbVisible] = useState(false);
  const hovered = useRef(false);
  const hideTimer = useRef<number | undefined>(undefined);
  const drag = useRef<{ x: number; scrollLeft: number } | null>(null);
  const followActive = useRef(true);

  const measure = useCallback(() => {
    const strip = stripRef.current;
    if (!strip) return;
    const ratio = strip.clientWidth / strip.scrollWidth;
    setOverflowing(ratio < 1);
    const thumb = thumbRef.current;
    if (!thumb) return;
    thumb.style.width = `${strip.clientWidth * ratio}px`;
    thumb.style.transform = `translateX(${strip.scrollLeft * ratio}px)`;
  }, []);

  const revealActiveTab = useCallback(() => {
    const strip = stripRef.current;
    const tab = strip?.querySelector<HTMLElement>(".tab.active");
    if (!strip || !tab) return;
    if (tab.offsetLeft < strip.scrollLeft) strip.scrollLeft = tab.offsetLeft;
    else if (tab.offsetLeft + tab.offsetWidth > strip.scrollLeft + strip.clientWidth) {
      strip.scrollLeft = tab.offsetLeft + tab.offsetWidth - strip.clientWidth;
    }
  }, []);

  const flashThumb = useCallback(() => {
    setThumbVisible(true);
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (!hovered.current) setThumbVisible(false);
    }, 1000);
  }, []);

  useEffect(() => () => window.clearTimeout(hideTimer.current), []);

  useEffect(() => {
    const strip = stripRef.current;
    const content = strip?.firstElementChild;
    if (!strip || !content) return;
    const stripObserver = new ResizeObserver(measure);
    // Tabs load and relabel after mount; keep the active one in view until the user scrolls away.
    const contentObserver = new ResizeObserver(() => {
      if (followActive.current) revealActiveTab();
      measure();
    });
    stripObserver.observe(strip);
    contentObserver.observe(content);
    return () => {
      stripObserver.disconnect();
      contentObserver.disconnect();
    };
  }, [measure, revealActiveTab]);

  // Size the thumb once it mounts; the first measure ran before it existed.
  useEffect(() => {
    if (overflowing) {
      measure();
      return;
    }
    hovered.current = false;
    drag.current = null;
    window.clearTimeout(hideTimer.current);
    setThumbVisible(false);
  }, [overflowing, measure]);

  useEffect(() => {
    followActive.current = true;
    revealActiveTab();
  }, [activeKey, revealActiveTab]);

  return (
    <div className="relative flex-1 min-w-0">
      {/* `relative` makes the strip the tabs' offsetParent for the reveal math. */}
      <div
        ref={stripRef}
        className="tab-strip relative overflow-x-auto overflow-y-hidden [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        onScroll={() => {
          measure();
          flashThumb();
        }}
        onWheel={(event) => {
          if (event.ctrlKey) return;
          followActive.current = false;
          if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
          const strip = event.currentTarget;
          strip.scrollLeft += event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? strip.clientWidth : 1);
        }}
      >
        <div className="flex items-end w-max">{children}</div>
      </div>
      {overflowing && (
        <div
          ref={thumbRef}
          aria-hidden="true"
          className={`absolute start-0 -bottom-px flex items-end h-1 ${thumbVisible ? "" : "pointer-events-none"}`}
          onPointerEnter={() => {
            hovered.current = true;
          }}
          onPointerLeave={() => {
            hovered.current = false;
            flashThumb();
          }}
          onPointerDown={(event) => {
            const strip = stripRef.current;
            if (!strip) return;
            event.preventDefault();
            followActive.current = false;
            event.currentTarget.setPointerCapture(event.pointerId);
            drag.current = { x: event.clientX, scrollLeft: strip.scrollLeft };
          }}
          onPointerMove={(event) => {
            const strip = stripRef.current;
            if (!strip || !drag.current) return;
            const scale = strip.scrollWidth / strip.clientWidth;
            strip.scrollLeft = drag.current.scrollLeft + (event.clientX - drag.current.x) * scale;
          }}
          onLostPointerCapture={() => {
            drag.current = null;
          }}
        >
          <div className={`h-0.5 w-full rounded-full bg-muted transition-opacity duration-300 motion-reduce:transition-none ${thumbVisible ? "opacity-100" : "opacity-0"}`} />
        </div>
      )}
    </div>
  );
}
