import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RuntimeRoot } from "./RemoteRuntime";
import { Toaster } from "./components/ui";
import { getLocale } from "./paraglide/runtime.js";
import "./tailwind.css";

const locale = getLocale();
document.documentElement.lang = locale;
document.documentElement.dir = "ltr";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RuntimeRoot />
    <Toaster />
  </StrictMode>,
);
