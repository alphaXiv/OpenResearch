import { paraglideVitePlugin } from "@inlang/paraglide-js";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { execFile } from "node:child_process";
import { cpSync, createReadStream, readFileSync, statSync } from "node:fs";
import type { ClientRequest, IncomingMessage } from "node:http";
import { basename, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { createLogger, defineConfig, type Plugin, type ProxyOptions } from "vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";

// Backend the dev server proxies to. Defaults to the standard `orx up` port;
// override with ORX_BACKEND when running against a backend on another port.
const backend = process.env.ORX_BACKEND ?? "http://127.0.0.1:4791";

// A restarting or stopped backend refuses each proxied request; the dashboard shows that itself.
const logger = createLogger();
const logError = logger.error;
logger.error = (message, options) => {
  const error = options?.error;
  if (error && "code" in error && error.code === "ECONNREFUSED") return;
  logError(message, options);
};

/**
 * The backend accepts only loopback Host headers and writes from its own origin, so a request from a
 * page this server served is passed on as the backend's own. Any other page keeps its Origin.
 */
const toBackend: ProxyOptions = {
  target: backend,
  ws: true,
  changeOrigin: true,
  configure(proxy) {
    const asBackend = (proxyReq: ClientRequest, req: IncomingMessage) => {
      if (req.headers.origin === `http://${req.headers.host}`) proxyReq.setHeader("origin", backend);
    };
    proxy.on("proxyReq", asBackend);
    proxy.on("proxyReqWs", asBackend);
    // A piped response stays open when the backend goes away mid-stream, so a page's event
    // stream would never learn to reconnect to the server that replaced it.
    proxy.on("proxyRes", (proxyRes, _req, res) => {
      proxyRes.on("close", () => {
        if (!res.writableEnded) res.destroy();
      });
    });
  },
};

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

const run = promisify(execFile);

/** Skips jj's empty, undescribed working-copy commit so the line names the work, not a placeholder. */
const JJ_REVSET = 'latest(heads(::@ ~ (empty() & description(exact:""))))';
const JJ_TEMPLATE = 'change_id.shortest(8) ++ "\\0" ++ bookmarks.join(" ") ++ "\\0" ++ description';

type ExecError = Error & { code?: unknown; stderr?: string };

/** jj when the checkout has a jj repo, git only when jj is missing or the checkout is git-only. */
async function readCommit(cwd: string) {
  try {
    const { stdout } = await run("jj", ["log", "--no-graph", "-r", JJ_REVSET, "-T", JJ_TEMPLATE], { cwd });
    const [id, ref, description] = stdout.split("\0");
    return { id, ref, description: description.trim() };
  } catch (error) {
    const { code, stderr, message } = error as ExecError;
    const noJj = code === "ENOENT" || stderr?.includes("There is no jj repo");
    if (!noJj) throw new Error(`jj could not read the commit in ${cwd}: ${stderr?.trim() || message}`);
  }

  try {
    const [{ stdout: log }, { stdout: branch }] = await Promise.all([
      run("git", ["log", "-1", "--format=%h%x00%B"], { cwd }),
      run("git", ["branch", "--show-current"], { cwd }),
    ]);
    const [id, description] = log.split("\0");
    return { id, ref: branch.trim(), description: description.trim() };
  } catch (error) {
    const { stderr, message } = error as ExecError;
    throw new Error(`git could not read the commit in ${cwd}: ${stderr?.trim() || message}`);
  }
}

/** The checkout's current commit at /__dev/commit, so dev windows from different worktrees can be told apart. */
function devCommit(): Plugin {
  return {
    name: "dev-commit",
    apply: "serve",
    // A post hook, so Vite's Host check runs first.
    configureServer: (server) => () => {
      server.middlewares.use("/__dev/commit", (req, res) => {
        res.setHeader("content-type", "application/json");

        // Reading snapshots the jj working copy, so other sites may not trigger it.
        const site = req.headers["sec-fetch-site"];
        if (site && site !== "same-origin") {
          res.statusCode = 403;
          res.end(JSON.stringify({ error: "Only this dev server's pages may read its commit." }));
          return;
        }

        readCommit(server.config.root).then(
          (commit) => res.end(JSON.stringify(commit)),
          (error: Error) => {
            res.statusCode = 500;
            res.end(JSON.stringify({ error: error.message }));
          },
        );
      });
    },
  };
}

export default defineConfig({
  customLogger: logger,
  plugins: [
    pdfjsAssets(),
    devCommit(),
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
    // T3 Code's preview browser reaches this machine's ports as <device>.knife.test.
    allowedHosts: [".knife.test"],
    proxy: { "/api": toBackend, "/_orx": toBackend, "/opencode": toBackend },
  },
});
