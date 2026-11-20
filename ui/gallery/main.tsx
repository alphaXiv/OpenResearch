import { createRoot } from "react-dom/client";
import { overwriteGetLocale } from "../src/paraglide/runtime.js";
import "../src/tailwind.css";
import { Gallery } from "./Gallery";
import { galleryFetch } from "./backend";
import "./gallery.css";

globalThis.fetch = galleryFetch;
overwriteGetLocale(() => "en");
createRoot(document.getElementById("root")!).render(<Gallery />);
