import { m } from "./paraglide/messages.js";
import { defineAppHotkeys, getHotkeyManager, toHotkeyRegistrationView } from "@tanstack/react-hotkeys";

export const appHotkeys = defineAppHotkeys({
  "task.new": { hotkey: "Mod+N", meta: { name: m.chat_panel_new_chat() } },
  "tab.close": { hotkey: "Mod+W", options: { capture: true, preventDefault: "always" }, meta: { name: m.closable_tab_close_tab() } },
  "tab.keepOpen": { hotkey: ["Mod+K", "Enter"], meta: { name: m.menu_keep_open() } },
  "app.quit": { hotkey: "Mod+Q", meta: { name: m.menu_quit() } },
  "window.close": { hotkey: "Mod+Shift+W", meta: { name: m.menu_close_window() } },
  "zoom-in": { hotkey: "Mod+=", meta: { name: m.image_zoom_in() } },
  "zoom-out": { hotkey: "Mod+-", meta: { name: m.image_zoom_out() } },
  "zoom-reset": { hotkey: "Mod+0", meta: { name: m.menu_actual_size() } },
  "settings.open": { hotkey: "Mod+,", meta: { name: m.menu_settings() } },
  "compute.open": { hotkey: "Mod+Alt+C", meta: { name: m.settings_page_compute() } },
  "experiments.open": { hotkey: "Mod+Shift+E", meta: { name: m.app_experiments() } },
  "files.open": { hotkey: "Mod+Shift+F", meta: { name: m.app_files() } },
  "artifacts.open": { hotkey: "Mod+Shift+A", meta: { name: m.app_artifacts() } },
  "terminal.open": { hotkey: "Mod+Shift+T", meta: { name: m.workspace_terminal() } },
  "sidebar.toggle": { hotkey: "Mod+B", meta: { name: m.menu_toggle_sidebar() } },
  "tab.next": { hotkey: "Control+Tab", meta: { name: m.menu_next_tab() } },
  "tab.previous": { hotkey: "Control+Shift+Tab", meta: { name: m.menu_previous_tab() } },
  "tab.nextArrow": { hotkey: "Mod+Alt+ArrowRight", meta: { name: m.menu_next_tab() } },
  "tab.previousArrow": { hotkey: "Mod+Alt+ArrowLeft", meta: { name: m.menu_previous_tab() } },
  "tab.jump.1": { hotkey: "Mod+1", meta: { name: m.menu_tab_number({ number: 1 }) } },
  "tab.jump.2": { hotkey: "Mod+2", meta: { name: m.menu_tab_number({ number: 2 }) } },
  "tab.jump.3": { hotkey: "Mod+3", meta: { name: m.menu_tab_number({ number: 3 }) } },
  "tab.jump.4": { hotkey: "Mod+4", meta: { name: m.menu_tab_number({ number: 4 }) } },
  "tab.jump.5": { hotkey: "Mod+5", meta: { name: m.menu_tab_number({ number: 5 }) } },
  "tab.jump.6": { hotkey: "Mod+6", meta: { name: m.menu_tab_number({ number: 6 }) } },
  "tab.jump.7": { hotkey: "Mod+7", meta: { name: m.menu_tab_number({ number: 7 }) } },
  "tab.jump.8": { hotkey: "Mod+8", meta: { name: m.menu_tab_number({ number: 8 }) } },
  "tab.jump.9": { hotkey: "Mod+9", meta: { name: m.menu_tab_number({ number: 9 }) } },
});

export const tabJumpHotkeys = Object.values(appHotkeys).filter((hotkey) => hotkey.id.startsWith("tab.jump."));

declare module "@tanstack/react-hotkeys" {
  interface Register {
    appHotkeys: typeof appHotkeys;
  }
}

window.addEventListener("orx:command", (event) => {
  if (!(event instanceof CustomEvent)) return;
  if (typeof event.detail !== "string") return;
  if (event.detail.startsWith("native:")) {
    const action = event.detail.slice("native:".length);
    if (["undo", "redo", "cut", "copy", "paste", "selectAll"].includes(action)) document.execCommand(action);
    return;
  }
  const manager = getHotkeyManager();
  for (const [registrationId, registration] of manager.registrations.state) {
    if (registration.options.appHotkeyId !== event.detail) continue;
    if (toHotkeyRegistrationView(registration).status !== "active") continue;
    manager.triggerRegistration(registrationId);
    return;
  }
});
