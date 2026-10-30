import { formatForDisplay, type AppHotkey, type Hotkey } from "@tanstack/react-hotkeys";
import { appHotkeys, tabJumpHotkeys } from "./hotkeys";
import { m } from "./paraglide/messages.js";

type MenuEntry =
  | { kind: "command"; id: string; label: string; hotkey: Hotkey | null; checked?: boolean; macOnly?: boolean; windowsOnly?: boolean }
  | { kind: "native"; name: string; label?: string; hotkey?: Hotkey; macOnly?: boolean }
  | { kind: "separator" };

interface AppMenu {
  id: string;
  label: string;
  macOnly?: boolean;
  items: MenuEntry[];
}

function command(hotkey: AppHotkey, options: { checked?: boolean; macOnly?: boolean; windowsOnly?: boolean } = {}): MenuEntry {
  const { id, hotkey: keys, meta } = hotkey;
  if (!meta?.name) throw new Error(`Missing menu label: ${id}`);
  return { kind: "command", id, label: meta.name, hotkey: keys, ...options };
}

const appMenu: AppMenu[] = [
  { id: "app", label: "OpenResearch", macOnly: true, items: [
    { kind: "native", name: "about" },
    { kind: "separator" },
    command(appHotkeys["settings.open"]),
    { kind: "separator" },
    { kind: "native", name: "services" },
    { kind: "separator" },
    { kind: "native", name: "hide" },
    { kind: "native", name: "hide_others" },
    { kind: "native", name: "show_all" },
    { kind: "separator" },
    command(appHotkeys["app.quit"]),
  ] },
  { id: "file", label: m.menu_file(), items: [
    command(appHotkeys["task.new"]), command(appHotkeys["tab.keepOpen"]),
    command(appHotkeys["settings.open"], { windowsOnly: true }),
    { kind: "separator" },
    command(appHotkeys["tab.close"]), command(appHotkeys["window.close"], { macOnly: true }),
  ] },
  { id: "edit", label: m.menu_edit(), items: [
    { kind: "native", name: "undo", label: m.menu_undo(), hotkey: "Mod+Z" },
    { kind: "native", name: "redo", label: m.menu_redo(), hotkey: "Mod+Shift+Z" },
    { kind: "separator" },
    { kind: "native", name: "cut", label: m.menu_cut(), hotkey: "Mod+X" },
    { kind: "native", name: "copy", label: m.menu_copy(), hotkey: "Mod+C" },
    { kind: "native", name: "paste", label: m.menu_paste(), hotkey: "Mod+V" },
    { kind: "native", name: "select_all", label: m.menu_select_all(), hotkey: "Mod+A" },
  ] },
  { id: "view", label: m.menu_view(), items: [
    command(appHotkeys["sidebar.toggle"], { checked: false }),
    command(appHotkeys["compute.open"]),
    command(appHotkeys["experiments.open"]),
    command(appHotkeys["files.open"]), command(appHotkeys["artifacts.open"]), command(appHotkeys["terminal.open"]),
    { kind: "separator" },
    command(appHotkeys["zoom-in"]), command(appHotkeys["zoom-out"]), command(appHotkeys["zoom-reset"]),
    { kind: "native", name: "fullscreen", macOnly: true },
  ] },
  { id: "window", label: m.menu_window(), items: [
    command(appHotkeys["tab.next"]), command(appHotkeys["tab.previous"]),
    command(appHotkeys["tab.nextArrow"]), command(appHotkeys["tab.previousArrow"]),
    ...tabJumpHotkeys.map((hotkey) => command(hotkey)),
    { kind: "separator" },
    { kind: "native", name: "minimize", macOnly: true },
    { kind: "native", name: "maximize", macOnly: true },
  ] },
];

export function appMenuFor(platform: "mac" | "windows"): AppMenu[] {
  return appMenu.filter((section) => platform === "mac" || !section.macOnly).map((section) => {
    const items = section.items.filter((item) => {
      if (platform === "mac") return !("windowsOnly" in item && item.windowsOnly);
      return !("macOnly" in item && item.macOnly);
    });
    const visibleItems = items.filter((item, index) => item.kind !== "separator" || (index > 0 && index < items.length - 1 && items[index - 1].kind !== "separator"));

    return { ...section, items: visibleItems.map((item) => {
      if (platform === "mac" && item.kind === "command" && Array.isArray(item.hotkey)) {
        return { ...item, label: `${item.label} (${formatForDisplay(item.hotkey, { platform })})` };
      }
      return item;
    }) };
  });
}
