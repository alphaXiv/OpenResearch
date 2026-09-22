import assert from "node:assert/strict";
import test from "node:test";

import {
  chatImageTarget,
  isExternalMarkdownTarget,
  markdownTargetUrl,
  resolveMarkdownTarget,
} from "../src/markdownTarget.ts";

test("repository markdown resolves images relative to the document", () => {
  assert.deepEqual(resolveMarkdownTarget("", "dev/logo.png"), {
    path: "dev/logo.png",
    query: "",
    hash: "",
  });
  assert.deepEqual(resolveMarkdownTarget("docs/guides", "../../images/chart 1.png?raw=1#plot"), {
    path: "images/chart 1.png",
    query: "raw=1",
    hash: "#plot",
  });
  assert.deepEqual(resolveMarkdownTarget("docs", "/assets/logo.svg"), {
    path: "assets/logo.svg",
    query: "",
    hash: "",
  });
});

test("markdown paths cannot escape their root", () => {
  assert.equal(resolveMarkdownTarget("docs", "../../secret.png"), null);
  assert.equal(resolveMarkdownTarget("", "../secret.png"), null);
  assert.equal(resolveMarkdownTarget("", "%E0%A4%A"), null);
});

test("absolute markdown files preserve filesystem-rooted image paths", () => {
  assert.deepEqual(resolveMarkdownTarget("/tmp/reports", "../images/chart.png", true), {
    path: "/tmp/images/chart.png",
    query: "",
    hash: "",
  });
});

test("external image targets remain external", () => {
  assert.equal(isExternalMarkdownTarget("https://example.com/image.png"), true);
  assert.equal(isExternalMarkdownTarget("data:image/png;base64,AAAA"), true);
  assert.equal(isExternalMarkdownTarget("../images/chart.png"), false);
});

test("resolved image URLs preserve query parameters and fragments", () => {
  const target = resolveMarkdownTarget("docs", "image.png?raw=1#preview");
  assert.ok(target);
  assert.equal(
    markdownTargetUrl("/api/file/raw?path=docs%2Fimage.png", target),
    "/api/file/raw?path=docs%2Fimage.png&raw=1#preview",
  );
});

test("chat images resolve local paths without crossing the session root", () => {
  assert.deepEqual(chatImageTarget("paper/figures/plot%20one.png"), {
    path: "paper/figures/plot one.png", hash: "", source: "checkout",
  });
  assert.equal(chatImageTarget("/tmp/figure.png").source, "absolute");
  assert.equal(chatImageTarget("~/figures/figure.png").source, "absolute");
  assert.deepEqual(chatImageTarget("C:/papers/figure.png"), {
    path: "C:/papers/figure.png", hash: "", source: "absolute",
  });
  assert.deepEqual(chatImageTarget("artifacts/paper/figure.png"), {
    path: "paper/figure.png", hash: "", source: "artifact",
  });
  for (const src of ["../secret.png", "%E0%A4%A", "javascript:alert(1)", "data:text/html,test", "https://example.com/image.png"]) {
    assert.equal(chatImageTarget(src), null, src);
  }
});


test("chat image URLs discard injected query parameters and preserve SVG fragments", () => {
  assert.deepEqual(chatImageTarget("figure.svg?path=/secret&sessionId=other#diagram"), {
    path: "figure.svg", hash: "#diagram", source: "checkout",
  });
  assert.equal(chatImageTarget("figures/100%25.png").path, "figures/100%.png");
  assert.equal(chatImageTarget(String.raw`C:\papers\figure.png`).path, "C:/papers/figure.png");
});
