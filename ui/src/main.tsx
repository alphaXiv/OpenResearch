import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppRoot } from "./AppRoot";
import { getLocale } from "./paraglide/runtime.js";
import "./tailwind.css";

const locale = getLocale();
document.documentElement.lang = locale;
document.documentElement.dir = "ltr";

// A plain-HTTP LAN origin (`orx up --host 0.0.0.0`) is not a secure context, which has no randomUUID.
if (!("randomUUID" in crypto)) {
  Object.defineProperty(crypto, "randomUUID", {
    value: () => {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
  });
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppRoot />
  </StrictMode>,
);
