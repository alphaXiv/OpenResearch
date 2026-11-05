import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { readFileSync } from "node:fs";
import { createRequire, registerHooks } from "node:module";
import { after, afterEach } from "node:test";
import { fileURLToPath } from "node:url";
import { format } from "node:util";

const paraglide = new URL("../src/paraglide/", import.meta.url).href;
const compiledParaglide = new URL("../node_modules/.cache/paraglide/", import.meta.url).href;
const source = (url) => readFileSync(new URL(url), "utf8");
const require = createRequire(import.meta.url);
// esbuild comes with Vite, which compiles the app the same way.
const { transformSync } = createRequire(require.resolve("vite"))("esbuild");
const tsconfig = source(new URL("../tsconfig.json", import.meta.url));
const esm = (source) => ({ format: "module", source, shortCircuit: true });

registerHooks({
  resolve(specifier, context, nextResolve) {
    // Packages resolve to their browser builds, as Vite bundles them.
    context = { ...context, conditions: [...context.conditions, "browser"] };
    if (specifier.startsWith("@xterm/")) return { url: `${nextResolve(specifier, context).url}?umd`, shortCircuit: true };
    if (!specifier.startsWith(".") || !context.parentURL?.startsWith("file:")) return nextResolve(specifier, context);

    const url = new URL(specifier, context.parentURL).href;
    if (url.startsWith(paraglide)) return { url: compiledParaglide + url.slice(paraglide.length), shortCircuit: true };

    for (const suffix of ["", ".ts", ".tsx", "/index.ts"]) {
      try {
        return nextResolve(specifier + suffix, context);
      } catch (error) {
        if (error.code !== "ERR_MODULE_NOT_FOUND" && error.code !== "ERR_UNSUPPORTED_DIR_IMPORT") throw error;
        if (suffix === "/index.ts") throw error;
      }
    }
  },
  load(url, context, nextLoad) {
    if (url.endsWith("?umd")) {
      const path = fileURLToPath(url);
      const names = Object.keys(require(path)).join(", ");
      return esm(`import { createRequire } from "node:module"; const umd = createRequire(import.meta.url)(${JSON.stringify(path)}); export const { ${names} } = umd;`);
    }
    if (url.includes("/node_modules/") || !url.startsWith("file:")) return nextLoad(url, context);

    const { pathname, search } = new URL(url);
    if (search === "?raw") return esm(`export default ${JSON.stringify(source(url))};`);
    if (pathname.endsWith(".css")) return esm("");
    if (/\.(svg|png|jpe?g|gif|webp|woff2?)$/.test(pathname)) return esm(`export default ${JSON.stringify(pathname)};`);
    if (!/\.tsx?$/.test(pathname)) return nextLoad(url, context);

    const { code } = transformSync(source(url), {
      sourcefile: pathname,
      loader: pathname.endsWith("x") ? "tsx" : "ts",
      format: "esm",
      tsconfigRaw: tsconfig,
      // Vite's build-time constants, as a production build sees them.
      define: { "import.meta.env.DEV": "false" },
    });
    return esm(code);
  },
});

// Tests never reach the network: happy-dom loads no scripts, styles or frames, and the app's fetch fails unless a test serves it.
GlobalRegistrator.register({
  url: "http://localhost/",
  settings: {
    disableJavaScriptFileLoading: true,
    disableCSSFileLoading: true,
    handleDisabledFileLoadingAsSuccess: true,
    navigation: { disableMainFrameNavigation: true, disableChildFrameNavigation: true, disableChildPageNavigation: true },
  },
});
// happy-dom has no compatMode, which KaTeX reads as quirks mode.
Object.defineProperty(document, "compatMode", { value: "CSS1Compat" });
// happy-dom has no popover API, which tooltips open on focus.
HTMLElement.prototype.showPopover = HTMLElement.prototype.hidePopover = () => {};
after(() => GlobalRegistrator.unregister());

globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const actWarnings = [];
const { error } = console;
console.error = (...args) => {
  const message = format(...args);
  if (message.includes("not wrapped in act(")) actWarnings.push(message.split("\n")[0]);
  error(...args);
};
const failOnActWarnings = () => {
  if (actWarnings.length) throw new Error(actWarnings.splice(0).join("\n"));
};
afterEach(failOnActWarnings);
after(failOnActWarnings);

globalThis.fetch = async (input) => { throw new Error(`unexpected fetch: ${input}`); };
globalThis.WebSocket = class { constructor(url) { throw new Error(`unexpected WebSocket: ${url}`); } };
