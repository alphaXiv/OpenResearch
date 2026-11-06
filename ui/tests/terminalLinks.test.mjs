import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier !== "@xterm/addon-web-links") return nextResolve(specifier, context);
    return { url: "data:text/javascript,export class WebLinksAddon { constructor(handler) { globalThis.webLinkHandler = handler; } activate() {} dispose() {} }", shortCircuit: true };
  },
});
const { mountTerminal } = await import("../src/components/terminal.ts");

test("OSC 8 and plain terminal links open HTTP URLs without a confirmation", async (t) => {
  const opened = t.mock.method(window, "open", () => null);
  const { terminal, dispose } = mountTerminal(document.body.appendChild(document.createElement("div")), false, true);
  t.after(dispose);
  const { linkHandler } = terminal.options;
  linkHandler.activate({}, "https://example.com/sign-in");
  globalThis.webLinkHandler({}, "http://localhost:3000/callback");
  linkHandler.activate({}, "javascript:alert(1)");
  assert.deepEqual(opened.mock.calls.map(({ arguments: [url] }) => String(url)), ["https://example.com/sign-in", "http://localhost:3000/callback"]);
  assert.ok(opened.mock.calls.every(({ arguments: [, target, features] }) => target === "_blank" && features === "noopener,noreferrer"));

  terminal.resize(12, 4);
  await new Promise((resolve) => terminal.write("https://example.com/path", resolve));
  const range = { start: { x: 1, y: 1 }, end: { x: 12, y: 2 } };
  linkHandler.hover({}, "https://example.com/path", range);
  assert.equal(terminal.element.getAttribute("title"), null, "a visible URL needs no tooltip, even when wrapped");
  linkHandler.hover({}, "https://elsewhere.example", range);
  assert.equal(terminal.element.getAttribute("title"), "https://elsewhere.example");
  linkHandler.leave();
  assert.equal(terminal.element.getAttribute("title"), null);
});
