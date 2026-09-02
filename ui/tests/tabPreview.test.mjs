import assert from "node:assert/strict";
import test from "node:test";

import {
  closeTab,
  openIntentForKey,
  openTab,
  tabOpenGestureHandlers,
} from "../src/tabPreview.ts";

test("a preview replaces the previous preview in place", () => {
  const first = openTab({ order: ["kept", "old-preview", "tail"], previewKey: "old-preview" }, "next", "preview");
  assert.deepEqual(first, {
    order: ["kept", "next", "tail"],
    previewKey: "next",
    replacedKey: "old-preview",
  });
});

test("opening a kept tab leaves an unrelated preview available", () => {
  const next = openTab({ order: ["preview"], previewKey: "preview" }, "kept", "keepOpen");
  assert.deepEqual(next, {
    order: ["preview", "kept"],
    previewKey: "preview",
    replacedKey: null,
  });
});

test("reopening the current preview with keep-open intent promotes it", () => {
  const next = openTab({ order: ["preview"], previewKey: "preview" }, "preview", "keepOpen");
  assert.deepEqual(next, {
    order: ["preview"],
    previewKey: null,
    replacedKey: null,
  });
});

test("selecting an existing kept tab does not consume another preview", () => {
  const next = openTab(
    { order: ["kept", "preview"], previewKey: "preview" },
    "kept",
    "preview",
  );
  assert.deepEqual(next, {
    order: ["kept", "preview"],
    previewKey: "preview",
    replacedKey: null,
  });
});

test("closing a preview clears the slot without disturbing tab order", () => {
  assert.deepEqual(
    closeTab(
      { order: ["one", "preview", "two"], previewKey: "preview" },
      "preview",
      ["one", "two", "preview"],
    ),
    { order: ["one", "two"], previewKey: null, fallbackKey: "two" },
  );
});

test("closing the active tab falls back to the most recently selected remaining tab", () => {
  assert.equal(
    closeTab(
      { order: ["first", "active", "last"], previewKey: null },
      "active",
      ["last", "first", "active"],
    ).fallbackKey,
    "first",
  );
});

test("closing a kept tab leaves the current preview intact", () => {
  assert.deepEqual(
    closeTab(
      { order: ["kept", "preview"], previewKey: "preview" },
      "kept",
      ["preview", "kept"],
    ),
    { order: ["preview"], previewKey: "preview", fallbackKey: "preview" },
  );
});

test("a stale preview key is not reported as replaced", () => {
  assert.deepEqual(
    openTab({ order: ["kept"], previewKey: "stale" }, "next", "preview"),
    {
      order: ["kept", "next"],
      previewKey: "next",
      replacedKey: null,
    },
  );
});

test("switching tasks restores each saved preview slot and tab order", () => {
  const tasks = new Map();
  let active = openTab({ order: [], previewKey: null }, "a-preview", "preview");
  tasks.set("task-a", active);
  active = openTab({ order: ["b-kept"], previewKey: null }, "b-preview", "preview");
  tasks.set("task-b", active);
  active = tasks.get("task-a");

  assert.deepEqual(active, {
    order: ["a-preview"],
    previewKey: "a-preview",
    replacedKey: null,
  });
  assert.deepEqual(tasks.get("task-b"), {
    order: ["b-kept", "b-preview"],
    previewKey: "b-preview",
    replacedKey: null,
  });
});

test("VS Code tree keys map to preview and keep-open intent", () => {
  assert.equal(openIntentForKey(" "), "preview");
  assert.equal(openIntentForKey("Enter"), "keepOpen");
  assert.equal(openIntentForKey("Escape"), null);
});

test("shared gesture handlers map mouse and keyboard actions consistently", () => {
  const intents = [];
  const events = [];
  const event = (extra = {}) => ({
    preventDefault: () => events.push("prevent"),
    stopPropagation: () => events.push("stop"),
    ...extra,
  });
  const handlers = tabOpenGestureHandlers(
    (intent) => intents.push(intent),
    { stopPropagation: true },
  );

  handlers.onClick(event());
  handlers.onDoubleClick(event());
  handlers.onAuxClick(event({ button: 1 }));
  handlers.onKeyDown(event({ key: " " }));
  handlers.onKeyDown(event({ key: "Enter" }));

  assert.deepEqual(intents, ["preview", "keepOpen", "keepOpen", "preview", "keepOpen"]);
  assert.deepEqual(events, [
    "stop",
    "stop",
    "prevent", "stop",
    "prevent", "stop",
    "prevent", "stop",
  ]);
});
