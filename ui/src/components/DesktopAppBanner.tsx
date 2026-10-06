import { Download, X } from "lucide-react";
import { useEffect, useState } from "react";
import { m } from "../paraglide/messages.js";

import { IconButton } from "./ui";

const DISMISSED_KEY = "desktop-app-banner-dismissed-on";
const RELEASES = "https://github.com/alphaXiv/OpenResearch/releases/latest";

// Null when the platform names no asset: the viewer picks one from the release page.
function asset(): string | null {
  const platform = navigator.platform;
  if (/Mac/.test(platform)) return "OpenResearch.dmg";
  if (/Win/.test(platform)) return "OpenResearch-Setup.exe";
  if (/Linux x86_64/.test(platform)) return "OpenResearch-x86_64.AppImage";
  if (/Linux aarch64/.test(platform)) return "OpenResearch-aarch64.AppImage";
  return null;
}

// Dismissing lasts until the viewer's next calendar day.
function today(): string {
  return new Date().toDateString();
}

function dismissedToday(): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === today();
  } catch {
    return false;
  }
}

/** Points browser-dashboard users at the desktop app, which reads the same projects. */
export function DesktopAppBanner() {
  const [dismissed, setDismissed] = useState(dismissedToday);
  // Dashboards stay open for days: re-check when the tab comes back.
  useEffect(() => {
    const recheck = () => setDismissed(dismissedToday());
    window.addEventListener("focus", recheck);
    return () => window.removeEventListener("focus", recheck);
  }, []);
  if ("__ORX_DESKTOP__" in window || dismissed) return null;

  const dismiss = () => {
    try {
      localStorage.setItem(DISMISSED_KEY, today());
    } catch {
      // Storage is off: the banner returns on the next load.
    }
    setDismissed(true);
  };

  const file = asset();
  return (
    <div className="desktop-app-banner flex items-center gap-2 shrink-0 py-1.5 px-3.5 text-sm text-text bg-surface border-b border-b-border">
      <Download size={13} className="shrink-0 text-subtext" />
      <span className="min-w-0">{m.desktop_app_banner_text()}</span>
      <a
        href={file ? `${RELEASES}/download/${file}` : RELEASES}
        target={file ? undefined : "_blank"}
        rel="noreferrer"
        className="text-sm text-subtext underline shrink-0"
      >
        {m.desktop_app_banner_download()}
      </a>
      <IconButton type="button" size="small" className="ms-auto" aria-label={m.desktop_app_banner_dismiss()} onClick={dismiss}>
        <X size={13} />
      </IconButton>
    </div>
  );
}
