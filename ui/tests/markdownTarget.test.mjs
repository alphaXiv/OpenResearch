import assert from "node:assert/strict";
import test from "node:test";

import {
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
