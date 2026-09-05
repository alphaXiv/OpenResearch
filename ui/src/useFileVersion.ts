import { useEffect, useState } from "react";
import { onChatEvent } from "./events";

/** Only mounted previews probe their own file; unchanged media keeps its URL. */
export function useFileVersion(url: string, enabled = true) {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    let pending = false;
    const check = async () => {
      if (pending || document.visibilityState === "hidden") return;
      pending = true;
      try {
        const response = await fetch(url, {
          method: "HEAD",
          cache: "no-store",
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        if (response.status === 404) setVersion("missing");
        else if (response.ok) {
          // Older servers only expose size; never reset unchanged media on a timer.
          setVersion(response.headers.get("etag") ?? response.headers.get("content-length"));
        }
      } catch {
        // A transient disconnect leaves the preview intact; the next tick retries.
      } finally {
        pending = false;
      }
    };
    void check();
    const timer = window.setInterval(() => void check(), 2000);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    const off = onChatEvent((event) => {
      if (event.type === "reconnected") void check();
    });
    return () => {
      controller.abort();
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
      off();
    };
  }, [url, enabled]);

  return version;
}
