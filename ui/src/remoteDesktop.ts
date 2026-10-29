export function getLocalDesktopOrigin(): string | null {
  if (typeof window === "undefined") return null;
  const fromQuery = new URLSearchParams(window.location.search).get("local_origin");
  if (fromQuery) {
    try {
      sessionStorage.setItem("orx:local_desktop_origin", fromQuery);
    } catch {
      // sessionStorage unavailable
    }
    return fromQuery;
  }
  try {
    const stored = sessionStorage.getItem("orx:local_desktop_origin");
    if (stored) return stored;
  } catch {
    // sessionStorage unavailable
  }
  if ("__ORX_DESKTOP__" in window) {
    if (window.location.port !== "4792") {
      return "http://127.0.0.1:4792";
    }
  }
  return null;
}

export function returnToLocalWorkspace(localOrigin?: string | null) {
  const target = localOrigin || getLocalDesktopOrigin();
  if (target && typeof window !== "undefined") {
    window.location.href = target;
  }
}
