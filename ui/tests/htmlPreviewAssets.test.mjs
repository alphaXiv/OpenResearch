import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { HtmlPreview } from "../src/components/HtmlPreview.tsx";
import { render, serve, settle } from "./render.mjs";

test("external assets bypass the local resolver; relative ones keep the MIME gate", async () => {
  const types = { "/root/chart.png": "image/png", "/root/key.txt": "text/plain" };
  const fetched = [];
  serve((path) => {
    fetched.push(path);
    return new Response("data", { headers: { "content-type": types[path], "content-length": "4" } });
  });
  const resolved = [];
  const html = `<img src="https://cdn.example/a.png"><img src="//cdn.example/b.png"><script src="https://cdn.example/x.js"></script><img src="chart.png"><img src="key.txt">`;
  const { container } = await render(createElement(HtmlPreview, {
    html, truncated: false, url: "/root/page.html", name: "page.html",
    resolveSrc: (src) => { resolved.push(src); return `/root/${src}`; },
  }));
  await settle();

  assert.deepEqual(resolved, ["chart.png", "key.txt"]);
  assert.deepEqual(fetched, ["/root/chart.png", "/root/key.txt"]);
  const frame = new DOMParser().parseFromString(container.querySelector("iframe").getAttribute("srcdoc"), "text/html");
  assert.deepEqual([...frame.querySelectorAll("img, script")].map((element) => element.getAttribute("src")), [
    "https://cdn.example/a.png", "https://cdn.example/b.png", "https://cdn.example/x.js", "data:image/png;base64,ZGF0YQ==", "key.txt",
  ]);
});
