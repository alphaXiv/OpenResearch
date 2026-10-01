import { paraglideVitePlugin } from "@inlang/paraglide-js";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { cpSync, createReadStream, readFileSync, statSync } from "node:fs";
import { basename, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";

// Backend the dev server proxies to. Defaults to the standard `orx up` port;
// override with ORX_BACKEND when running against a backend on another port.
const backend = process.env.ORX_BACKEND ?? "http://127.0.0.1:4791";

const outDir = fileURLToPath(new URL("./dist/", import.meta.url));
const pdfjsDir = fileURLToPath(new URL("./node_modules/pdfjs-dist/", import.meta.url));
const pdfjsVersion: string = JSON.parse(readFileSync(join(pdfjsDir, "package.json"), "utf8")).version;
const PDFJS_DIRS = ["cmaps", "iccs", "standard_fonts", "wasm"];

// Every webview we ship in runs wasm, and PdfPreview never enables PDF scripting.
function wantedPdfjsFile(path: string): boolean {
  return !/^quickjs-eval\.|_nowasm_fallback\.js$/.test(basename(path));
}

/** PDF.js's character maps, fonts, and decoders, at /pdfjs/<version>/ for PdfPreview.tsx. */
function pdfjsAssets(): Plugin {
  return {
    name: "pdfjs-assets",
    configureServer(server) {
      server.middlewares.use(`/pdfjs/${pdfjsVersion}`, (req, res, next) => {
        const file = normalize(join(pdfjsDir, decodeURIComponent((req.url ?? "").split("?")[0])));
        const inside = PDFJS_DIRS.some((dir) => file.startsWith(join(pdfjsDir, dir) + sep));
        if (!inside || !wantedPdfjsFile(file) || !statSync(file, { throwIfNoEntry: false })?.isFile()) {
          next();
          return;
        }
        createReadStream(file).pipe(res);
      });
    },
    writeBundle() {
      for (const name of PDFJS_DIRS) {
        cpSync(join(pdfjsDir, name), join(outDir, "pdfjs", pdfjsVersion, name), {
          recursive: true,
          filter: wantedPdfjsFile,
        });
      }
    },
  };
}

export default defineConfig({
  plugins: [
    pdfjsAssets(),
    tanstackRouter({ target: "react", autoCodeSplitting: false }),
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
  build: { outDir },
  // PDF.js's worker is an ES module.
  worker: { format: "es" },
  server: {
    proxy: {
      "/api": { target: backend, ws: true },
      "/_orx": { target: backend, ws: true },
      "/opencode": backend,
    },
  },
});
