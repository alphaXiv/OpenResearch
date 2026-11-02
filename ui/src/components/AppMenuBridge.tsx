import { useEffect } from "react";
import { useHotkey, useHotkeyRegistrations, useHotkeys } from "@tanstack/react-hotkeys";
import { appMenuFor } from "../appMenu";
import { appHotkeys } from "../hotkeys";

export function AppMenuBridge() {
  const registrations = useHotkeyRegistrations();
  const state = JSON.stringify(Object.fromEntries(
    Object.keys(appHotkeys)
      .filter((id) => id !== "app.quit" && id !== "window.close" && !id.startsWith("zoom-"))
      .map((id) => [id, { enabled: registrations.some((item) => item.options.appHotkeyId === id && item.status === "active") }]),
  ));

  useHotkey("Mod+R", () => {}, { enabled: false, preventDefault: "always" });

  useHotkeys([
    { hotkey: appHotkeys["zoom-in"], callback: () => window.ipc?.postMessage("page-zoom:in") },
    { hotkey: appHotkeys["zoom-out"], callback: () => window.ipc?.postMessage("page-zoom:out") },
    { hotkey: appHotkeys["zoom-reset"], callback: () => window.ipc?.postMessage("page-zoom:reset") },
    { hotkey: "Mod++", callback: () => window.ipc?.postMessage("page-zoom:in") },
  ]);

  useEffect(() => {
    window.ipc?.postMessage(`menu:spec:${JSON.stringify(appMenuFor("mac"))}`);
  }, []);
  useEffect(() => {
    window.ipc?.postMessage(`menu:state:${state}`);
  }, [state]);
  return null;
}
