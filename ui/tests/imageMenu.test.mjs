import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement, Fragment } from "react";
import { ArtifactMarkdown } from "../src/components/ArtifactsTab.tsx";
import { ImageMenu } from "../src/components/ImageMenu.tsx";
import { ImageModal } from "../src/components/ImageModal.tsx";
import { ChatImageScope, Md } from "../src/components/Md.tsx";
import { MediaPreview } from "../src/components/MediaPreview.tsx";
import { MediaToolbarSlot } from "../src/components/mediaToolbar.tsx";
import { Toaster } from "../src/components/ui/Toast.tsx";
import { m } from "../src/paraglide/messages.js";
import { assistantMessage, renderChat, session, userMessage } from "./chat.mjs";
import { find, press, render, settle } from "./render.mjs";

const withToasts = (element) => render(createElement(Fragment, null, createElement(Toaster), element));

const png = new Blob([new Uint8Array([137, 80, 78, 71])], { type: "image/png" });

// happy-dom draws nothing, so the canvas stands in for the browser's encoder.
function fakeCanvas(t, { context = { drawImage() {} } } = {}) {
  t.mock.method(HTMLCanvasElement.prototype, "getContext", () => context);
  t.mock.method(HTMLCanvasElement.prototype, "toBlob", (done) => done(png));
  t.mock.getter(HTMLImageElement.prototype, "naturalWidth", () => 640);
  t.mock.getter(HTMLImageElement.prototype, "naturalHeight", () => 480);
}

function recordWrites(t, write = async () => {}) {
  const writes = [];
  t.mock.method(navigator.clipboard, "write", async (items) => {
    for (const item of items) writes.push(Object.fromEntries(await Promise.all(item.types.map(async (type) => [type, await item.getType(type)]))));
    return write();
  });
  return writes;
}

async function openMenu(trigger) {
  await act(() => trigger.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2 })));
  await settle();
  return find('[role="menu"]');
}

async function choose(label) {
  await act(async () => find('[role="menuitem"]', label).click());
  await settle();
}

test("copying an agent's figure puts only the image on the clipboard", async (t) => {
  fakeCanvas(t);
  const writes = recordWrites(t);
  const { container } = await withToasts(createElement(ChatImageScope, { projectId: "p" }, createElement(Md, { text: "![Loss curve](figures/loss.png)" })));
  const image = container.querySelector('img[alt="Loss curve"]');

  const menu = await openMenu(image);
  assert.equal(menu.getAttribute("aria-label"), m.image_menu());
  await choose(m.image_copy());

  assert.deepEqual(writes.map((item) => Object.keys(item)), [["image/png"]]);
  assert.equal(writes[0]["image/png"].type, "image/png");
  assert.match(document.body.textContent, new RegExp(m.common_copied()));
});

test("a failed copy says so", async (t) => {
  fakeCanvas(t);
  recordWrites(t, async () => { throw new Error("Write permission denied."); });
  const { container } = await withToasts(bareImage());

  await openMenu(container.querySelector("img"));
  await choose(m.image_copy());

  assert.match(document.body.textContent, new RegExp(m.image_copy_failed()));
  assert.match(document.body.textContent, /Write permission denied/);
});

const bareImage = () => createElement(ImageMenu, { src: "/a.png", name: "a.png" }, createElement("img", { src: "/a.png", alt: "" }));

test("copy starts the clipboard write inside the click, before the image is encoded", async (t) => {
  fakeCanvas(t);
  let decoded;
  t.mock.method(HTMLImageElement.prototype, "decode", () => new Promise((resolve) => { decoded = resolve; }));
  const items = [];
  t.mock.method(navigator.clipboard, "write", (written) => { items.push(...written); return Promise.resolve(); });
  const { container } = await withToasts(bareImage());

  await openMenu(container.querySelector("img"));
  await choose(m.image_copy());

  assert.deepEqual(items.map((item) => item.types), [["image/png"]]);
  const blob = items[0].getType("image/png");
  await act(async () => decoded());
  assert.equal((await blob).type, "image/png");
});

test("an image that didn't load says so instead of copying nothing", async (t) => {
  fakeCanvas(t);
  t.mock.method(HTMLImageElement.prototype, "decode", async () => { throw new DOMException("broken", "EncodingError"); });
  const writes = recordWrites(t);
  const { container } = await withToasts(bareImage());

  await openMenu(container.querySelector("img"));
  await choose(m.image_copy());

  assert.deepEqual(writes, []);
  assert.match(document.body.textContent, new RegExp(m.image_copy_not_loaded()));
});

test("an image too large to draw points to Save image", async (t) => {
  fakeCanvas(t, { context: null });
  recordWrites(t);
  const { container } = await withToasts(bareImage());

  await openMenu(container.querySelector("img"));
  await choose(m.image_copy());

  assert.match(document.body.textContent, new RegExp(m.image_copy_too_large()));
});

test("save downloads the image under its name, from a link wrapping it", async (t) => {
  const clicks = [];
  t.mock.method(HTMLAnchorElement.prototype, "click", function () {
    if (this.hasAttribute("download")) clicks.push([this.getAttribute("href"), this.getAttribute("download")]);
  });
  const { container } = await render(createElement(ImageMenu, { src: "/api/chat/attachments/plot.png", name: "plot.png" },
    createElement("a", { href: "/api/chat/attachments/plot.png" }, createElement("img", { src: "/api/chat/attachments/plot.png", alt: "" }))));

  await openMenu(container.querySelector("a"));
  await choose(m.image_save());

  assert.deepEqual(clicks, [["/api/chat/attachments/plot.png", "plot.png"]]);
});

test("an artifact's image saves under its own file name", async (t) => {
  const clicks = [];
  t.mock.method(HTMLAnchorElement.prototype, "click", function () {
    if (this.hasAttribute("download")) clicks.push(this.getAttribute("download"));
  });
  const { container } = await render(createElement(ArtifactMarkdown, { projectId: "p", folder: "", markdown: "![Plot](figures/loss.png)", entries: [] }));

  await openMenu(container.querySelector("img"));
  await choose(m.image_save());

  assert.deepEqual(clicks, ["loss.png"]);
});

test("an image file's preview copies it from the toolbar", async (t) => {
  fakeCanvas(t);
  const writes = recordWrites(t);
  const slot = document.createElement("div");
  document.body.append(slot);
  t.after(() => slot.remove());
  await render(createElement(MediaToolbarSlot, { value: slot }, createElement(MediaPreview, { kind: "image", url: "/plot.png", name: "plot.png" })));

  await act(async () => slot.querySelector(`button[aria-label="${m.image_copy()}"]`).click());
  await settle();

  assert.deepEqual(writes.map((item) => Object.keys(item)), [["image/png"]]);
});

test("the image menu opens from the keyboard and returns focus on Escape", async () => {
  const { container } = await render(createElement(ImageMenu, { src: "/a.png", name: "a.png" }, createElement("button", { type: "button" }, createElement("img", { src: "/a.png", alt: "Plot" }))));
  const trigger = container.querySelector("button");
  await act(() => trigger.focus());

  await press("F10", trigger, { shiftKey: true });
  assert.ok(find('[role="menuitem"]', m.image_copy()));
  await press("Escape");
  await settle();
  assert.equal(find('[role="menu"]'), null);
  assert.equal(document.activeElement, trigger);
});

test("the image viewer copies the image it shows", async (t) => {
  fakeCanvas(t);
  const writes = recordWrites(t);
  await render(createElement(ImageModal, { src: "/a.png", alt: "Plot", name: "a.png", onClose() {} }));
  const image = find('[role="dialog"] img');
  await act(() => image.dispatchEvent(new Event("load")));

  await act(async () => find(`button[aria-label="${m.image_copy()}"]`).click());
  await settle();

  assert.deepEqual(writes.map((item) => Object.keys(item)), [["image/png"]]);
});

test("moving through a figure's menu leaves the chat where it was", async () => {
  await renderChat("/projects/p/tasks/s", {
    sessions: [session("s")],
    transcripts: { s: [userMessage("u1", "Plot it"), assistantMessage("a1", "![Loss curve](figures/loss.png)", "u1")] },
  });
  const row = find('[role="article"] img[alt="Loss curve"]').closest('[role="article"]');
  // A focused row scrolls itself into view, so the row must never take focus here.
  const rowFocus = [];
  row.addEventListener("focus", () => rowFocus.push(document.activeElement));

  await openMenu(row.querySelector("img"));
  const copy = find('[role="menuitem"]', m.image_copy());
  await act(() => copy.focus());
  await settle();

  assert.deepEqual(rowFocus, []);
  assert.equal(document.activeElement, copy);
});
