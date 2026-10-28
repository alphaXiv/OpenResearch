import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/components/InlineHtmlFigure.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;

// Minimal hook runtime: enough to run InlineHtmlFigure's effect and re-render on state changes.
function mount(props) {
  const slots = [];
  let index = 0;
  let tree;
  const pending = [];
  const react = {
    useState: (initial) => {
      const slot = index++;
      if (!(slot in slots)) slots[slot] = initial;
      return [slots[slot], (value) => { slots[slot] = value; render(); }];
    },
    useEffect: (effect, deps) => {
      const slot = index++;
      const previous = slots[slot];
      if (previous && deps.every((dep, i) => dep === previous.deps[i])) return;
      slots[slot] = { deps };
      pending.push(() => { previous?.cleanup?.(); slots[slot].cleanup = effect(); });
    },
  };
  const HtmlPreview = () => null;
  const element = (type, elementProps) => ({ type, props: elementProps });
  const exports = {};
  new Function("require", "exports", compiled)((name) => ({
    react,
    "react/jsx-runtime": { jsx: element, jsxs: element },
    "../paraglide/messages.js": { m: new Proxy({}, { get: (_, key) => () => String(key) }) },
    "./HtmlPreview": { HtmlPreview },
  })[name], exports);
  function render() {
    index = 0;
    tree = exports.InlineHtmlFigure(props);
    while (pending.length) pending.shift()();
  }
  render();
  const find = (node) => !node?.props ? undefined : node.type === HtmlPreview ? node : [node.props.children].flat().map(find).find(Boolean);
  return { preview: () => find(tree)?.props, rerender: (next) => { props = next; render(); }, text: () => JSON.stringify(tree) };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const response = (status, type, body = "") => ({ ok: status === 200, headers: { get: () => type }, text: async () => body });

test("the fallback candidate that served the HTML resolves its assets", async (t) => {
  const fetched = [];
  const replies = { "/a": response(404, "text/html"), "/b": response(200, "image/png"), "/c": response(200, "text/html; charset=utf-8", "<p>c</p>"), "/d": response(200, "text/html") };
  t.mock.method(globalThis, "fetch", async (url) => { fetched.push(url); return replies[url]; });
  const candidates = ["/a", "/b", "/c", "/d"].map((url) => ({ url, resolveAsset: (src) => `${url}:${src}` }));
  const view = mount({ candidates, name: "chart" });
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
  const view = mount({ candidates: [{ url: "/old", resolveAsset: () => null }], name: "chart" });
  view.rerender({ candidates: [{ url: "/x", resolveAsset: () => null }, { url: "/y", resolveAsset: () => null }], name: "chart" });
  assert.equal(signals[0].aborted, true);
  await settle();
  assert.equal(signals.length, 3);
  assert.equal(view.preview(), undefined);
  assert.match(view.text(), /file_viewer_failed_to_load_file/);
  assert.match(view.text(), /"href":"\/x"/);
});
