import assert from "node:assert/strict";
import test from "node:test";
import { readClipboardImage } from "../src/clipboardImage.ts";

function clipboardWith(items) {
  return {
    read: async () => items,
  };
}

function imageItem(type, bytes) {
  return {
    types: [type],
    getType: async () => new Blob([new Uint8Array(bytes)], { type }),
  };
}

test("returns the first non-empty image as a File", async () => {
  const file = await readClipboardImage(
    clipboardWith([
      {
        types: ["text/plain"],
        getType: async () => new Blob(["text"], { type: "text/plain" }),
      },
      imageItem("image/png", [1, 2, 3]),
    ]),
  );
  assert.equal(file.name, "pasted-image.png");
  assert.equal(file.type, "image/png");
  assert.equal(file.size, 3);
});

test("uses the jpg extension for jpeg images", async () => {
  const file = await readClipboardImage(clipboardWith([imageItem("image/jpeg", [1])]));
  assert.equal(file.name, "pasted-image.jpg");
  assert.equal(file.type, "image/jpeg");
});

test("skips empty images and non-image items", async () => {
  const file = await readClipboardImage(
    clipboardWith([
      imageItem("image/png", []),
      {
        types: ["application/pdf"],
        getType: async () => new Blob(["pdf"], { type: "application/pdf" }),
      },
      imageItem("image/webp", [4]),
    ]),
  );
  assert.equal(file.name, "pasted-image.webp");
  assert.equal(file.size, 1);
});

test("returns null when the clipboard has no image", async () => {
  const file = await readClipboardImage(
    clipboardWith([
      {
        types: ["text/plain"],
        getType: async () => new Blob(["text"], { type: "text/plain" }),
      },
    ]),
  );
  assert.equal(file, null);
});

test("returns null when reading is denied", async () => {
  const file = await readClipboardImage({
    read: async () => {
      throw new Error("NotAllowedError");
    },
  });
  assert.equal(file, null);
});

test("returns null without a clipboard reader", async () => {
  assert.equal(await readClipboardImage(undefined), null);
});
