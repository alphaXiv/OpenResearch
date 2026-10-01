import assert from "node:assert/strict";
import test from "node:test";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";

import {
  chatImageTarget,
  firstCitedLine,
  isExternalMarkdownTarget,
  markdownTargetUrl,
  rehypeSafeUrls,
  resolveMarkdownTarget,
  splitLineSuffix,
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

test("file citations split off their line suffix", () => {
  assert.deepEqual(splitLineSuffix("src/foo.py:42"), { path: "src/foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("foo.py:42:7"), { path: "foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("Makefile:42"), { path: "Makefile", line: 42 });
  assert.deepEqual(splitLineSuffix("foo.py:42-50"), { path: "foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("src/foo.py#L42"), { path: "src/foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("src/foo.py#L42C3-L50C1"), { path: "src/foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("src/foo.py"), { path: "src/foo.py" });
  assert.deepEqual(splitLineSuffix("docs/guide.md#setup"), { path: "docs/guide.md#setup" });
  assert.deepEqual(splitLineSuffix("C:/repo/foo.py"), { path: "C:/repo/foo.py" });
  assert.deepEqual(splitLineSuffix("C:/repo/foo.py:42"), { path: "C:/repo/foo.py", line: 42 });
  assert.deepEqual(splitLineSuffix("foo.py:0"), { path: "foo.py" });
  assert.deepEqual(splitLineSuffix("#L42"), { path: "#L42" });
});

test("cited line ranges resolve to their first line", () => {
  assert.equal(firstCitedLine("20"), 20);
  assert.equal(firstCitedLine("20-40"), 20);
  assert.equal(firstCitedLine("L20-L40"), 20);
  assert.equal(firstCitedLine("x"), undefined);
  assert.equal(firstCitedLine("-5"), undefined);
});

test("cited hrefs are sanitized and carried out of band", () => {
  const processor = unified().use(remarkParse).use(remarkRehype).use(rehypeSafeUrls);
  const link = (target) => processor.runSync(processor.parse(`[x](${target})`)).children[0].children[0].properties;
  assert.deepEqual(link("Makefile:42"), { href: "", "data-cited-href": "Makefile:42" });
  assert.deepEqual(link("src/foo.py:42"), { href: "src/foo.py:42", "data-cited-href": "src/foo.py:42" });
  assert.deepEqual(link("javascript:1"), { href: "", "data-cited-href": "javascript:1" });
  assert.equal(link("javascript:alert(1)//x.py:1").href, "");
  assert.deepEqual(link("https://example.com/docs"), { href: "https://example.com/docs" });
});
