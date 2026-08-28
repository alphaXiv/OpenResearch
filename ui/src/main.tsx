import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { getLocale, getTextDirection } from "./paraglide/runtime.js";
import "./tailwind.css";

const locale = getLocale();
document.documentElement.lang = locale;
document.documentElement.dir = getTextDirection(locale);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
