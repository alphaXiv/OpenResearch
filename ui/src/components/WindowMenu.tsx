import { useEffect, useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Menubar } from "@base-ui/react/menubar";
import { formatForDisplay, useHeldKeys, useHotkeyRegistrations } from "@tanstack/react-hotkeys";
import { appMenuFor } from "../appMenu";
import { m } from "../paraglide/messages.js";
import { EscapeScope } from "./EscapeScope";

export function WindowMenu() {
  const [visible, setVisible] = useState(false);
  const held = useHeldKeys();
  const wasAlt = useRef(false);
  const usedAlt = useRef(false);
  const previousFocus = useRef<HTMLElement | null>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const registrations = useHotkeyRegistrations();

  const dismiss = () => {
    setVisible(false);
    previousFocus.current?.focus();
  };

  useEffect(() => {
    const alt = held.includes("Alt");
    if (alt && !wasAlt.current) {
      previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      usedAlt.current = false;
      setVisible(true);
    }
    if (alt && held.some((key) => key !== "Alt")) usedAlt.current = true;
    if (!alt && wasAlt.current) {
      if (usedAlt.current) setVisible(false);
      else barRef.current?.querySelector<HTMLElement>("button")?.focus();
    }
    wasAlt.current = alt;
  }, [held]);

  useEffect(() => {
    if (!visible) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Element && event.target.closest("[role=menubar], [data-window-menu]")) return;
      setVisible(false);
    };
    const blur = () => setVisible(false);
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("blur", blur);
    };
  }, [visible]);

  if (!visible) return null;

  return (
    <EscapeScope onEscape={dismiss} exclusive={false}>
      <Menubar
        ref={barRef}
        aria-label={m.menu_menu_bar()}
        modal={false}
        className="pointer-events-auto fixed start-0 top-0 z-300 flex h-8 items-center border-b border-border bg-background text-sm text-text shadow-control-subtle"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            dismiss();
          }
        }}
        onBlur={(event) => {
          if (event.relatedTarget instanceof Element && !event.relatedTarget.closest("[role=menubar], [data-window-menu]")) setVisible(false);
        }}
      >
        {appMenuFor("windows").map((section) => (
          <Menu.Root key={section.id} onOpenChange={(open, details) => {
            if (!open && (details.reason === "escape-key" || details.reason === "outside-press")) dismiss();
          }}>
            <Menu.Trigger className="h-8 px-3 outline-none hover:bg-surface data-popup-open:bg-surface focus-visible:bg-surface">{section.label}</Menu.Trigger>
            <Menu.Portal>
              <Menu.Positioner side="bottom" align="start" className="z-300">
                <Menu.Popup data-window-menu className="min-w-56 rounded-md border border-border bg-background p-1 text-sm text-text shadow-menu outline-none" finalFocus={false}>
                  {section.items.map((item, index) => {
                    if (item.kind === "separator") return <Menu.Separator key={index} className="my-1 border-t border-border" />;
                    const enabled = item.kind === "native" || registrations.some((registration) => registration.options.appHotkeyId === item.id && registration.status === "active");
                    const checked = item.kind === "command" && item.checked !== undefined ? !!document.querySelector(".session-rail") : undefined;
                    const label = item.label;
                    const id = item.kind === "command" ? item.id : `native:${item.name === "select_all" ? "selectAll" : item.name}`;
                    return (
                      <Menu.Item key={id} disabled={!enabled} role={checked === undefined ? "menuitem" : "menuitemcheckbox"} aria-checked={checked}
                        className="flex items-center justify-between gap-6 rounded-sm px-2 py-1.5 outline-none data-highlighted:bg-surface data-disabled:text-muted"
                        onClick={() => {
                          dismiss();
                          window.dispatchEvent(new CustomEvent("orx:command", { detail: id }));
                        }}
                      >
                        <span>{checked ? "✓ " : ""}{label}</span>
                        {item.hotkey && <span className="text-xs text-subtext">{formatForDisplay(item.hotkey)}</span>}
                      </Menu.Item>
                    );
                  })}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        ))}
      </Menubar>
    </EscapeScope>
  );
}
