import { m } from "../paraglide/messages.js";

export function mediaUnavailable() {
  return window.__ORX_MEDIA_DISABLED__ === true;
}

export function MediaUnavailable() {
  return <div role="status" className="file-view-note shrink-0 border-b border-border-variant px-4 py-2 text-sm text-subtext">
    <p>{m.media_preview_install_runtime()}</p>
    <details className="mt-2">
      <summary>{m.media_preview_install_commands()}</summary>
      <p className="mt-2">Arch Linux</p>
      <pre className="overflow-x-auto whitespace-pre-wrap" dir="ltr">sudo pacman -S --needed webkit2gtk-4.1 gst-plugins-base-libs gst-plugins-good gst-libav</pre>
      <p className="mt-2">Ubuntu / Debian</p>
      <pre className="overflow-x-auto whitespace-pre-wrap" dir="ltr">sudo apt install libwebkit2gtk-4.1-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav</pre>
    </details>
  </div>;
}
