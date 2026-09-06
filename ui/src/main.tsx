import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
import { router } from "./router";
import { Toaster } from "./components/ui";
import { getLocale } from "./paraglide/runtime.js";
import "./tailwind.css";

const locale = getLocale();
document.documentElement.lang = locale;
document.documentElement.dir = "ltr";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RouterProvider router={router} />
    <Toaster />
  </StrictMode>,
);
