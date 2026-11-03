import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { QueryClient, QueryObserver } from "@tanstack/react-query";

const source = readFileSync(new URL("../src/components/InlineHtmlFigure.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

function mount(props, t) {
  let tree;
  let observer;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } });
  t.after(() => { observer?.destroy(); client.clear(); });
  const useQuery = (options) => {
    if (!observer) {
      observer = new QueryObserver(client, options);
      observer.subscribe(() => queueMicrotask(render));
    } else observer.setOptions(options);
    return observer.getCurrentResult();
  };
  const HtmlPreview = () => null;
  const element = (type, elementProps) => ({ type, props: elementProps });
  const exports = {};
  new Function("require", "exports", compiled)((name) => ({
    "@tanstack/react-query": { useQuery },
    "../workspace": { useWorkspace: () => ({ workspaceKey: (...key) => ["machine", ...key], request: (...args) => fetch(...args) }) },
    "react/jsx-runtime": { jsx: element, jsxs: element },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => () => String(key) }) },
    "./HtmlPreview": { HtmlPreview },
  })[name], exports);
  function render() {
    tree = exports.InlineHtmlFigure(props);
  }
  render();
  const find = (node) => !node?.props ? undefined : node.type === HtmlPreview ? node : [node.props.children].flat().map(find).find(Boolean);
  return { hide: () => observer.destroy(), show: () => { observer = undefined; render(); }, preview: () => find(tree)?.props, rerender: (next) => { props = next; render(); }, text: () => JSON.stringify(tree) };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const response = (status, type, body = "") => ({ ok: status === 200, headers: { get: () => type }, text: async () => body });

test("the fallback candidate that served the HTML resolves its assets", async (t) => {
  const fetched = [];
  const replies = { "/a": response(404, "text/html"), "/b": response(200, "image/png"), "/c": response(200, "text/html; charset=utf-8", "<p>c</p>"), "/d": response(200, "text/html") };
  t.mock.method(globalThis, "fetch", async (url) => { fetched.push(url); return replies[url]; });
  const candidates = ["/a", "/b", "/c", "/d"].map((url) => ({ url, resolveAsset: (src) => `${url}:${src}` }));
  const view = mount({ candidates, name: "chart" }, t);
  await settle();
  const preview = view.preview();
  assert.deepEqual(fetched, ["/a", "/b", "/c"]);
  assert.equal(preview.url, "/c");
  assert.equal(preview.html, "<p>c</p>");
  assert.equal(preview.resolveSrc, candidates[2].resolveAsset);
});

test("changing candidates aborts the previous load, and all-failed shows a bounded error", async (t) => {
  const signals = [];
  t.mock.method(globalThis, "fetch", async (_url, { signal }) => { signals.push(signal); return response(404, "text/html"); });
  const view = mount({ candidates: [{ url: "/old", resolveAsset: () => null }], name: "chart" }, t);
  view.rerender({ candidates: [{ url: "/x", resolveAsset: () => null }, { url: "/y", resolveAsset: () => null }], name: "chart" }, t);
  assert.equal(signals[0].aborted, true);
  await settle();
  assert.equal(signals.length, 3);
  assert.equal(view.preview(), undefined);
  assert.match(view.text(), /file_viewer_failed_to_load_file/);
  assert.match(view.text(), /"href":"\/x"/);
});

test("returning to a cached figure shows it immediately without another request", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => { calls++; return response(200, "text/html", "<p>retained</p>"); });
  const view = mount({ candidates: [{ url: "/figure", resolveAsset: () => null }], name: "chart" }, t);
  await settle();
  assert.equal(view.preview().html, "<p>retained</p>");
  view.hide();
  view.show();
  assert.equal(view.preview().html, "<p>retained</p>");
  await settle();
  assert.equal(calls, 1);
});
