import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { MediaPreview } from "../src/components/MediaPreview.tsx";
import { HtmlPreview } from "../src/components/HtmlPreview.tsx";
import { m } from "../src/paraglide/messages.js";
import { render, settle } from "./render.mjs";

test("a missing desktop media runtime leaves downloads and installation instructions", async () => {
  window.__ORX_MEDIA_DISABLED__ = true;
  try {
    for (const kind of ["audio", "video"]) {
      const view = await render(createElement(MediaPreview, {kind, url:"/clip", name:"clip"}));
      assert.equal(view.container.querySelector("video, audio"), null);
      assert.ok(view.container.querySelector('[role="status"]').textContent.includes(m.media_preview_install_runtime()));
      assert.ok(view.container.querySelector("details summary").textContent.includes(m.media_preview_install_commands()));
      assert.ok(!view.container.textContent.includes(m.media_preview_this_browser_can_t_preview_this_media_format()));
      assert.equal(view.container.querySelector("a[download]").getAttribute("href"), "/clip");
      await view.unmount();
    }
    const view = await render(createElement(MediaPreview, {kind:"image", url:"/image", name:"figure"}));
    assert.ok(view.container.querySelector("img"));
    assert.equal(view.container.querySelector('[role="status"]'), null);
  } finally { delete window.__ORX_MEDIA_DISABLED__; }
});
test("HTML remains readable and sandboxed when desktop media is disabled", async () => {
  window.__ORX_MEDIA_DISABLED__ = true;
  try {
    const view=await render(createElement(HtmlPreview,{html:"<h1>Report remains available</h1>",truncated:false,url:"/report.html",name:"Report",resolveSrc:()=>null}));
    await settle();
    assert.ok(view.container.querySelector('[role="status"]'));
    const frame=view.container.querySelector("iframe");
    assert.ok(frame.getAttribute("srcdoc").includes("Report remains available"));
    assert.equal(frame.getAttribute("sandbox"),"allow-scripts allow-popups allow-downloads");
  } finally { delete window.__ORX_MEDIA_DISABLED__; }
});
test("a browser retains its normal media controls", async () => {
  const view=await render(createElement(MediaPreview,{kind:"video",url:"/clip",name:"clip"}));
  assert.ok(view.container.querySelector("video[controls]"));
  assert.equal(view.container.querySelector('[role="status"]'),null);
});
