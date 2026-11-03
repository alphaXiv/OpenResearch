import { AppMenuBridge } from "./components/AppMenuBridge";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { HotkeysProvider } from "@tanstack/react-hotkeys";
import { appHotkeys } from "./hotkeys";
import { Machines } from "./Machines";

import { Toaster } from "./components/ui";
import { DesktopZoomControls } from "./components/DesktopZoomControls";

import { WindowControls } from "./components/WindowControls";
import { WINDOWS_TITLEBAR } from "./desktopTitlebar";
import { getLocale } from "./paraglide/runtime.js";
import "./tailwind.css";

const locale = getLocale();
document.documentElement.lang = locale;
document.documentElement.dir = "ltr";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <HotkeysProvider appHotkeys={appHotkeys}>
      {"__ORX_DESKTOP__" in window && <AppMenuBridge />}
      <Machines />
      {WINDOWS_TITLEBAR && <WindowControls />}
    </HotkeysProvider>
    <Toaster />
    {"__ORX_DESKTOP__" in window && <DesktopZoomControls />}
  </StrictMode>,
);
