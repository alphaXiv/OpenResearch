import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const source = readFileSync(new URL("../../src/commands/app.rs", import.meta.url), "utf8");
const script = source.match(/const MACOS_ESCAPE_SCRIPT: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(script, "the macOS webview installs an Escape handler");

test("the Escape guard also reaches sandboxed preview frames", () => {
  assert.match(source, /\.with_initialization_script_for_main_only\(MACOS_ESCAPE_SCRIPT, false\)/);
  assert.match(source, /\.with_initialization_script\("window\.__ORX_MAC_TITLEBAR__ = true;"\)/);
});

function dispatch(key, { modal = false, cancelPrevented = false, popover = false, handled = false, modalSupported = true, popoverSupported = true, search = false, readOnly = false, disabled = false } = {}) {
  let listener;
  const result = { hidden: false, cancelled: false, closed: false };
  const dialog = {
    dispatchEvent(event) {
      assert.equal(event.type, "cancel");
      assert.equal(event.cancelable, true);
      result.cancelled = true;
      return !cancelPrevented;
    },
    close() { result.closed = true; },
  };
  runInNewContext(script, {
    window: {
      addEventListener(type, handler, capture = false) {
        assert.equal(type, "keydown");
        assert.equal(capture, false, "run after document-level chat/overlay handlers");
        listener = handler;
      },
    },
    CSS: {
      supports(selector) {
        if (selector === "selector(:modal)") return modalSupported;
        assert.equal(selector, "selector(:popover-open)");
        return popoverSupported;
      },
    },
    Event: class {
      constructor(type, { cancelable }) { Object.assign(this, { type, cancelable }); }
    },
    document: {
      querySelectorAll(selector) {
        if (selector === "dialog:modal" || selector === "dialog[open]") {
          if (selector === "dialog:modal" && !modalSupported) throw new SyntaxError("unsupported :modal");
          return modal ? [{}, dialog] : [];
        }
        assert.equal(selector, "[popover]:popover-open:not([popover=manual])");
        if (!popoverSupported) throw new SyntaxError("unsupported :popover-open");
        return popover && popover !== "manual" ? [{}, { hidePopover() { result.hidden = true; } }] : [];
      },
    },
  });
  const event = {
    key,
    defaultPrevented: handled,
    target: {
      matches(selector) {
        assert.equal(selector, "input[type=search]:enabled:not([readonly])");
        return search && !readOnly && !disabled;
      },
    },
    preventDefault() { this.defaultPrevented = true; },
  };
  listener(event);
  return { prevented: event.defaultPrevented, ...result };
}

const idle = { prevented: true, hidden: false, cancelled: false, closed: false };

test("unhandled Escape cannot invoke WebKit's native fullscreen exit", () => {
  assert.deepEqual(dispatch("Escape"), idle);
});

test("chat and overlay Escape handling is left alone", () => {
  assert.deepEqual(dispatch("Escape", { handled: true, modal: true, popover: true }), idle);
});

test("modal dialogs are cancelled without the native fallback", () => {
  assert.deepEqual(dispatch("Escape", { modal: true }), { ...idle, cancelled: true, closed: true });
});

test("dialogs that veto cancel stay open", () => {
  assert.deepEqual(dispatch("Escape", { modal: true, cancelPrevented: true }), { ...idle, cancelled: true });
});

test("automatic popovers are dismissed before their dialog, without the native fallback", () => {
  assert.deepEqual(dispatch("Escape", { popover: true, modal: true }), { ...idle, hidden: true });
});

test("manual tooltips are not dismissed and do not enable native fullscreen exit", () => {
  assert.deepEqual(dispatch("Escape", { popover: "manual" }), idle);
});

test("other keyboard defaults remain intact", () => {
  assert.equal(dispatch("Enter").prevented, false);
  assert.equal(dispatch("f").prevented, false);
});

test("editable search fields keep WebKit's handled Escape-to-clear default", () => {
  assert.equal(dispatch("Escape", { search: true }).prevented, false);
});

test("immutable search fields cannot enable native fullscreen exit", () => {
  assert.deepEqual(dispatch("Escape", { search: true, readOnly: true }), idle);
  assert.deepEqual(dispatch("Escape", { search: true, disabled: true }), idle);
});

test("older WebKit without optional selectors still prevents idle Escape", () => {
  assert.deepEqual(dispatch("Escape", { popoverSupported: false }), idle);
  assert.deepEqual(dispatch("Escape", { modalSupported: false, popoverSupported: false }), idle);
});

test("modal cancellation works when popover selectors are unavailable", () => {
  assert.deepEqual(dispatch("Escape", { modal: true, popoverSupported: false }), { ...idle, cancelled: true, closed: true });
});

test("WebKit with native dialogs but no :modal still cancels open dialogs", () => {
  assert.deepEqual(dispatch("Escape", { modal: true, modalSupported: false, popoverSupported: false }), { ...idle, cancelled: true, closed: true });
});
