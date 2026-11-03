import assert from "node:assert/strict";
import test from "node:test";
import {
  deleteDesktopClipboardTemp,
  desktopClipboardBridgeAvailable,
  desktopClipboardFileUrl,
  fileFromDesktopClipboardPayload,
  installDesktopClipboardBridge,
  requestDesktopClipboardPaste,
} from "../src/desktopClipboard.ts";

function installFakeWindow() {
  const messages = [];
  globalThis.window = {
    __ORX_CLIPBOARD_BRIDGE__: true,
    ipc: {
      postMessage(message) {
        messages.push(message);
      },
    },
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
  };
  return messages;
}

test("bridge is unavailable without a desktop ipc channel", () => {
  delete globalThis.window;
  assert.equal(desktopClipboardBridgeAvailable(), false);
});

test("native paste resolves the payload for its request id", async () => {
  const messages = installFakeWindow();
  const pending = requestDesktopClipboardPaste();
  const id = messages[0].slice("clipboard-paste:".length);
  const payload = {
    id,
    path: "/tmp/openresearch-clipboard/1-1.png",
    name: "pasted-image.png",
    mediaType: "image/png",
    size: 3,
  };
  window.__ORX_CLIPBOARD_PASTE__(payload);
  assert.deepEqual(await pending, payload);
});

test("native errors resolve null", async () => {
  const messages = installFakeWindow();
  const pending = requestDesktopClipboardPaste();
  const id = messages[0].slice("clipboard-paste:".length);
  window.__ORX_CLIPBOARD_PASTE__({ id, error: "no image" });
  assert.equal(await pending, null);
});

test("a missing native response times out", async () => {
  installFakeWindow();
  assert.equal(await requestDesktopClipboardPaste(5), null);
});

test("late native replies delete their temporary file", () => {
  const messages = installFakeWindow();
  installDesktopClipboardBridge();
  const path = "/tmp/openresearch-clipboard/1-late.png";
  window.__ORX_CLIPBOARD_PASTE__({
    id: "late",
    path,
    name: "pasted-image.png",
    mediaType: "image/png",
    size: 3,
  });
  assert.ok(messages.includes(`clipboard-temp-delete:${path}`));
});

test("clipboard temp files are fetched and deleted", async () => {
  const messages = installFakeWindow();
  const payload = {
    id: "1",
    path: "/tmp/openresearch-clipboard/1-1.png",
    name: "pasted-image.png",
    mediaType: "image/png",
    size: 3,
  };
  const file = await fileFromDesktopClipboardPayload(payload, async (url) => {
    assert.equal(
      url,
      `/api/files/abs/raw?path=${encodeURIComponent(payload.path)}`,
    );
    return new Response(new Uint8Array([1, 2, 3]), {
      headers: { "content-type": "image/png" },
    });
  });
  assert.equal(file.name, "pasted-image.png");
  assert.equal(file.type, "image/png");
  assert.equal(file.size, 3);
  assert.ok(messages.includes(`clipboard-temp-delete:${payload.path}`));
});

test("empty clipboard files are rejected and cleaned up", async () => {
  const messages = installFakeWindow();
  const payload = {
    id: "2",
    path: "/tmp/openresearch-clipboard/1-2.png",
    name: "pasted-image.png",
    mediaType: "image/png",
    size: 0,
  };
  const file = await fileFromDesktopClipboardPayload(
    payload,
    async () => new Response(new Uint8Array()),
  );
  assert.equal(file, null);
  assert.ok(messages.includes(`clipboard-temp-delete:${payload.path}`));
});

test("delete is ignored without the desktop bridge", () => {
  delete globalThis.window;
  assert.doesNotThrow(() => deleteDesktopClipboardTemp("/tmp/a.png"));
});

test("absolute file urls encode the temp path", () => {
  assert.equal(
    desktopClipboardFileUrl("/tmp/a b.png"),
    "/api/files/abs/raw?path=%2Ftmp%2Fa%20b.png",
  );
});
