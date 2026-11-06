import { HotkeysProvider } from "@tanstack/react-hotkeys";
import { AppMenuBridge } from "./components/AppMenuBridge";
import { DesktopZoomControls } from "./components/DesktopZoomControls";
import { Toaster } from "./components/ui";
import { WindowControls } from "./components/WindowControls";
import { WINDOWS_TITLEBAR } from "./desktopTitlebar";
import { appHotkeys } from "./hotkeys";
import { Machines } from "./Machines";

/** The whole page: every machine's workspace and the overlays that outlive them. */
export function AppRoot() {
  return <>
    <HotkeysProvider appHotkeys={appHotkeys}>
      {"__ORX_DESKTOP__" in window && <AppMenuBridge />}
      <Machines />
      {WINDOWS_TITLEBAR && <WindowControls />}
    </HotkeysProvider>
    <Toaster />
    {"__ORX_DESKTOP__" in window && <DesktopZoomControls />}
  </>;
}
