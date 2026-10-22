import assert from "node:assert/strict";
import test from "node:test";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";

import {
  chatImageTarget,
  chatImageTargets,
  citedFilePath,
  documentAssetTarget,
  encodeMarkdownPath,
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

test("markdown paths decode percent-encoded reserved filename characters once", () => {
  assert.deepEqual(resolveMarkdownTarget("docs", "figures/chart%231.png"), {
    path: "docs/figures/chart#1.png",
    query: "",
    hash: "",
  });
  assert.equal(resolveMarkdownTarget("", "a%3Fb%26c%3Ad.png")?.path, "a?b&c:d.png");
  assert.deepEqual(resolveMarkdownTarget("docs", "chart%231.png?raw=1#plot"), {
    path: "docs/chart#1.png",
    query: "raw=1",
    hash: "#plot",
  });
  assert.equal(resolveMarkdownTarget("docs", "chart%25231.png")?.path, "docs/chart%231.png");
  assert.deepEqual(chatImageTarget("figures/chart%231.png"), {
    path: "figures/chart#1.png",
    hash: "",
    source: "checkout",
  });
});

test("re-resolved markdown paths are decoded only once", () => {
  for (const name of ["chart#1.png", "chart%231.png", "a?b&c:d.png", "100%.png", "my figs/a b.png"]) {
    assert.equal(resolveMarkdownTarget("", encodeMarkdownPath(name))?.path, name);
  }
  const docLink = (target) => resolveMarkdownTarget("docs", target)?.path ?? null;
  assert.equal(citedFilePath("foo%2523bar.py", docLink), "docs/foo%23bar.py");
  assert.equal(citedFilePath("foo%23bar.py", docLink), "docs/foo#bar.py");
  assert.equal(citedFilePath("foo%2523bar.py"), "foo%23bar.py");
  assert.equal(citedFilePath("%E0%A4%A"), null);
  const figure = chatImageTarget("figures/chart%231.png");
  assert.equal(docLink(encodeMarkdownPath(figure.path)), "docs/figures/chart#1.png");
});

test("document assets resolve relative to the decoded document path once", () => {
  const asset = (path, src) => documentAssetTarget(path, src, true);
  assert.deepEqual(asset("my figs/plot.html", "chart%231.png#x"), { path: "my figs/chart#1.png", query: "", hash: "#x" });
  assert.equal(asset("figs/plot.html", "chart%25231.png").path, "figs/chart%231.png");
  assert.equal(asset("figs/a%25b/plot.html", "c.png").path, "figs/a%25b/c.png");
  assert.equal(asset("figs/plot.html", "a%3Fb.png").path, "figs/a?b.png");
  assert.equal(asset("plot.html", "a%3Ab.png").path, "a:b.png");
  assert.equal(asset("/tmp/r/plot.html", "../a%231.png").path, "/tmp/a#1.png");
  assert.equal(asset("C:/figs/plot.html", "chart.png").path, "C:/figs/chart.png");
  for (const src of ["%E0%A4%A", "a%00.png", "../../x.png"]) assert.equal(asset("figs/plot.html", src), null, src);
});

test("markdown -> nested HTML -> parent-relative image resolves against the Markdown folder first", () => {
  const html = resolveMarkdownTarget("/tmp/reports", "run/chart.html", true);
  assert.equal(html.path, "/tmp/reports/run/chart.html");
  assert.equal(documentAssetTarget(html.path, "../../shared.png", true).path, "/tmp/shared.png");
  const stored = resolveMarkdownTarget("reports", "run/chart.html");
  assert.equal(documentAssetTarget(stored.path, "../../shared.png").path, "shared.png");
  assert.equal(documentAssetTarget(stored.path, "../../../shared.png"), null);
});

test("markdown paths cannot escape their root", () => {
  assert.equal(resolveMarkdownTarget("docs", "../../secret.png"), null);
  assert.equal(resolveMarkdownTarget("", "../secret.png"), null);
  assert.equal(resolveMarkdownTarget("", "%E0%A4%A"), null);
  assert.equal(resolveMarkdownTarget("", "..%2Fsecret.png"), null);
  assert.equal(resolveMarkdownTarget("", "a%00.png"), null);
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
  for (const src of ["C%3A/papers/figure.png", "C%3A%5Cpapers%5Cfigure.png"]) {
    assert.deepEqual(chatImageTarget(src), {
      path: "C:/papers/figure.png", hash: "", source: "absolute",
    }, src);
  }
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

test("chat artifact images retain existing lookup order before trying a nested artifacts folder", () => {
  for (const ext of ["html", "svg", "png"]) {
    const path = `tiny-learning-rate-comparison/chart.${ext}`;
    assert.deepEqual(chatImageTargets(`artifacts/${path}`), [
      { path, hash: "", source: "artifact" },
      { path: `artifacts/${path}`, hash: "", source: "checkout" },
      { path: `artifacts/${path}`, hash: "", source: "artifact" },
    ]);
  }
  assert.deepEqual(chatImageTargets("artifacts/chart%25231.svg?path=/secret#plot"), [
    { path: "chart%231.svg", hash: "#plot", source: "artifact" },
    { path: "artifacts/chart%231.svg", hash: "#plot", source: "checkout" },
    { path: "artifacts/chart%231.svg", hash: "#plot", source: "artifact" },
  ]);
});

test("HTML assets resolve in the root of the candidate that served the document", () => {
  const [artifact, checkout, nested] = chatImageTargets("artifacts/run/chart.html");
  const asset = (figure, src) => documentAssetTarget(figure.path, src, true)?.path ?? null;
  assert.deepEqual([artifact, checkout, nested].map(figure => asset(figure, "chart%231.png")),
    ["run/chart#1.png", "artifacts/run/chart#1.png", "artifacts/run/chart#1.png"]);
  assert.equal(asset(artifact, "../shared.png"), "shared.png");
  assert.equal(asset(artifact, "../../shared.png"), null);
  assert.equal(asset(checkout, "../../shared.png"), "shared.png");
  assert.equal(asset(nested, "../../shared.png"), "shared.png");
  assert.deepEqual(documentAssetTarget(nested.path, "chart%25231.svg?path=/secret#plot", true),
    { path: "artifacts/run/chart%231.svg", query: "path=/secret", hash: "#plot" });
  for (const src of ["../../../secret.png", "..%2F..%2F..%2Fsecret.png", "%E0%A4%A", "a%00.png"]) {
    assert.equal(asset(nested, src), null, src);
  }
});

test("non-artifact images do not gain fallbacks and invalid paths stay rejected", () => {
  for (const src of ["paper/figure.png", "/tmp/figure.png", "~/figure.png", "C:/papers/figure.png"]) {
    assert.deepEqual(chatImageTargets(src), [chatImageTarget(src)]);
  }
  for (const src of ["../secret.png", "artifacts/../../secret.png", "%E0%A4%A", "javascript:1", "https://example.com/image.png"]) {
    assert.deepEqual(chatImageTargets(src), [], src);
  }
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
