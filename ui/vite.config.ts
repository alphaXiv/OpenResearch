import { paraglideVitePlugin } from "@inlang/paraglide-js";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

// Backend the dev server proxies to. Defaults to the standard `orx up` port;
// override with ORX_BACKEND when running against a backend on another port.
const backend = process.env.ORX_BACKEND ?? "http://127.0.0.1:4791";

export default defineConfig({
  plugins: [
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/paraglide",
      outputStructure: "message-modules",
      emitTsDeclarations: true,
      localStorageKey: "orx:locale",
      strategy: ["localStorage", "preferredLanguage", "baseLocale"],
    }),
    react(),
    tailwindcss(),
  ],
  build: { outDir: "dist" },
  server: {
    proxy: {
      "/api": { target: backend, ws: true },
      "/_orx": { target: backend, ws: true },
      "/opencode": backend,
    },
  },
});
