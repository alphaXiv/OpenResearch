import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { Md } from "../src/components/Md.tsx";
import { nativeMarkdownPath } from "../src/markdownTarget.ts";
import { render } from "./render.mjs";

/** Renders `[label](target)` and returns its link element and the files a click opens. */
async function link(target, { open = true, resolveFilePath } = {}) {
  const opened = [];
  const onOpenFile = open ? (path, line) => opened.push({ path, line }) : undefined;
  const { container } = await render(createElement(Md, { text: `[label](${target})`, onOpenFile, resolveFilePath }));
  const element = container.querySelector("p").firstElementChild;
  if (element?.tagName === "BUTTON") await act(() => element.click());
  return { element, opened };
}

test("local links dispatch native paths for PDF and other file types", async () => {
  for (const target of ["C:/report.pdf", "file:///C:/notes.txt", "file:///tmp/data.csv", "file:///tmp/plot.png"]) {
    const { element, opened } = await link(target, { resolveFilePath: (path) => `docs/${path}` });
    assert.equal(element.tagName, "BUTTON", target);
    assert.deepEqual(opened.map(({ path }) => path), [nativeMarkdownPath(target)]);
  }
});

test("relative links keep document context and source-line citations", async () => {
  assert.deepEqual((await link("src/main.rs:42", { resolveFilePath: (path) => `docs/${path}` })).opened, [{ path: "docs/src/main.rs", line: 42 }]);
  assert.deepEqual((await link("C:/src/main.rs:42")).opened, [{ path: "C:/src/main.rs", line: 42 }]);
});

test("rejected, empty and handlerless file links cannot open duplicate tabs", async () => {
  for (const target of ["", "javascript:alert(1)", "vscode://file/a.txt", "file://server/share/a.pdf", "file:///tmp/%00", "file:///tmp/%ZZ"]) {
    const { element, opened } = await link(target);
    assert.equal(element?.tagName ?? "SPAN", "SPAN", target);
    assert.deepEqual(opened, []);
  }
  for (const target of ["C:/report.pdf", "file:///tmp/a.txt", "relative.txt"]) {
    assert.equal((await link(target, { open: false })).element.tagName, "SPAN", target);
  }
  for (const target of ["https://example.com/a.pdf", "mailto:test@example.com", "//example.com/a.pdf"]) {
    const { element } = await link(target);
    assert.equal(element.tagName, "A");
    assert.equal(element.getAttribute("href"), target);
  }
});
