import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";

import { CodeEditor } from "../src/components/CodeEditor.tsx";
import { find, render } from "./render.mjs";

test("the highlighted text stops at the textarea's scrollbar, so the selection covers the lines it shows", async (t) => {
  // happy-dom lays nothing out: give the textarea a classic 10px scrollbar.
  const { prototype } = HTMLTextAreaElement;
  Object.defineProperty(prototype, "offsetWidth", { configurable: true, get: () => 360 });
  Object.defineProperty(prototype, "clientWidth", { configurable: true, get: () => 350 });
  t.after(() => {
    delete prototype.offsetWidth;
    delete prototype.clientWidth;
  });

  await render(createElement(CodeEditor, { value: "tasks = [\n  \"a\",\n]\n", onChange() {}, onSave() {}, path: "spec.toml" }));

  assert.equal(find(".file-view-code").style.insetInlineEnd, "10px");
});
