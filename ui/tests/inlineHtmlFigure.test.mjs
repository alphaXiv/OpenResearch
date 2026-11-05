import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { InlineHtmlFigure } from "../src/components/InlineHtmlFigure.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, serve, settle } from "./render.mjs";

const page = (status, type, body = "") => new Response(body, { status, headers: { "content-type": type } });

test("the fallback candidate that served the HTML resolves its assets", async () => {
  const fetched = [];
  const replies = {
    "/a": () => page(404, "text/html"),
    "/b": () => page(200, "image/png"),
    "/c": () => page(200, "text/html; charset=utf-8", '<p>c</p><img src="plot.png">'),
    "/d": () => page(200, "text/html"),
    "/c:plot.png": () => new Response("png", { headers: { "content-type": "image/png", "content-length": "3" } }),
  };
  serve((path) => { fetched.push(path); return replies[path]?.(); });
  const candidates = ["/a", "/b", "/c", "/d"].map((url) => ({ url, resolveAsset: (src) => `${url}:${src}` }));
  const { container } = await render(createElement(InlineHtmlFigure, { candidates, name: "chart" }));
  await settle();

  assert.deepEqual(fetched, ["/a", "/b", "/c", "/c:plot.png"]);
  const frame = new DOMParser().parseFromString(container.querySelector("iframe").getAttribute("srcdoc"), "text/html");
  assert.equal(frame.querySelector("p").textContent, "c");
  assert.match(frame.querySelector("img").getAttribute("src"), /^data:image\/png/);
});

test("changing candidates aborts the previous load, and all-failed shows a bounded error", async () => {
  const signals = [];
  serve((path, init) => {
    signals.push(init.signal);
    return path === "/old" ? new Promise(() => {}) : page(404, "text/html");
  });
  const { container, rerender } = await render(createElement(InlineHtmlFigure, { candidates: [{ url: "/old", resolveAsset: () => null }], name: "chart" }));
  await rerender(createElement(InlineHtmlFigure, { candidates: [{ url: "/x", resolveAsset: () => null }, { url: "/y", resolveAsset: () => null }], name: "chart" }));
  assert.equal(signals[0].aborted, true);
  await settle();

  assert.equal(signals.length, 3);
  assert.equal(container.querySelector("iframe")?.outerHTML, undefined);
  const link = container.querySelector("a");
  assert.equal(link.textContent, m.file_viewer_failed_to_load_file());
  assert.equal(link.getAttribute("href"), "/x");
});

test("returning to a cached figure shows it immediately without another request", async () => {
  let calls = 0;
  serve(() => { calls++; return page(200, "text/html", "<p>retained</p>"); });
  const figure = createElement(InlineHtmlFigure, { candidates: [{ url: "/figure", resolveAsset: () => null }], name: "chart" });
  const shown = (container) => new DOMParser().parseFromString(container.querySelector("iframe").getAttribute("srcdoc"), "text/html").querySelector("p").textContent;
  const first = await render(figure);
  await settle();
  assert.equal(shown(first.container), "retained");
  await first.unmount();

  const { container } = await render(figure, first.workspace);
  assert.equal(shown(container), "retained");
  await settle();
  assert.equal(calls, 1);
});
