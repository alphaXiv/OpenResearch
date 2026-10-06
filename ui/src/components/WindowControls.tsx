import { useSyncExternalStore, type MouseEvent } from "react";
import { m } from "../paraglide/messages.js";

// ChromeMinimize, ChromeMaximize, ChromeRestore, ChromeClose: shared by Segoe Fluent
// Icons (Windows 11) and Segoe MDL2 Assets (Windows 10).
const GLYPH = { minimize: "\uE921", maximize: "\uE922", restore: "\uE923", close: "\uE8BB" };

function subscribeMaximized(onChange: () => void) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-maximized"] });
  return () => observer.disconnect();
}

function send(message: string) {
  window.ipc?.postMessage(`titlebar:${message}`);
}

function resizeFrom(edge: string) {
  return (event: MouseEvent) => {
    if (event.button !== 0) return;
    // Keeps desktopTitlebar.ts from also starting a window drag.
    event.preventDefault();
    send(`resize:${edge}`);
  };
}

function CaptionButton({ label, glyph, message, className }: { label: string; glyph: string; message: string; className: string }) {
  return (
    <button
      className={`inline-flex h-8 w-11.5 items-center justify-center text-xs text-text font-['Segoe_Fluent_Icons','Segoe_MDL2_Assets'] ${className}`}
      // Native caption buttons aren't in the Tab order.
      tabIndex={-1}
      title={label}
      aria-label={label}
      onClick={() => send(message)}
    >
      {glyph}
    </button>
  );
}

/** The Windows app's caption buttons and top resize edge, which its frameless window lacks. */
export function WindowControls() {
  const maximized = useSyncExternalStore(subscribeMaximized, () =>
    document.documentElement.hasAttribute("data-maximized"),
  );
  return (
    <>
      {!maximized && (
        <>
          <div className="fixed inset-x-0 top-0 z-300 h-1 cursor-ns-resize" onMouseDown={resizeFrom("n")} />
          <div className="fixed start-0 top-0 z-300 size-2 cursor-nwse-resize" onMouseDown={resizeFrom("nw")} />
          <div className="fixed end-0 top-0 z-300 size-2 cursor-nesw-resize" onMouseDown={resizeFrom("ne")} />
        </>
      )}
      {/* Above the z-200 dialogs, so the window can always be minimized or closed. */}
      <div className="fixed end-0 top-0 z-300 flex">
        <CaptionButton label={m.window_minimize()} glyph={GLYPH.minimize} message="minimize" className="hover:bg-panel" />
        <CaptionButton
          label={maximized ? m.window_restore() : m.window_maximize()}
          glyph={maximized ? GLYPH.restore : GLYPH.maximize}
          message="zoom"
          className="hover:bg-panel"
        />
        <CaptionButton label={m.window_close()} glyph={GLYPH.close} message="close" className="hover:bg-accent-red hover:text-white" />
      </div>
    </>
  );
}
