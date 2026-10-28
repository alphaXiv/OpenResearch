import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import ts from "typescript";

const { outputText } = ts.transpileModule(readFileSync(new URL("../src/components/useDialogFocus.ts", import.meta.url), "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
});

function mount(controls) {
  let cleanup, keydown;
  const document = { activeElement: null };
  class Control {
    constructor(options = {}) { Object.assign(this, { tabIndex: 0, disabled: false, hidden: false, inert: false }, options); }
    matches(selector) { return selector === ":disabled" ? this.disabled : this.initial === true; }
    closest(selector) { assert.equal(selector, "[inert]"); return this.inert ? this : null; }
    getClientRects() { return this.hidden ? [] : [{}]; }
    focus() { document.activeElement = this; }
  }
  const previous = new Control();
  document.activeElement = previous;
  const children = controls.map((options) => new Control(options));
  const dialog = new Control();
  Object.assign(dialog, {
    querySelector: () => null,
    querySelectorAll: (selector) => { assert.ok(selector.includes("a[href]")); return children; },
    contains: (node) => children.includes(node) || node === dialog,
    addEventListener: (type, listener) => { assert.equal(type, "keydown"); keydown = listener; },
    removeEventListener: (type, listener) => { assert.equal(type, "keydown"); assert.equal(listener, keydown); },
  });
  const exports = {};
  new Function("require", "exports", "document", "HTMLElement", "getComputedStyle", outputText)(
    () => ({ useEffect: (effect) => { cleanup = effect(); } }), exports, document, Control, () => ({ visibility: "visible" }),
  );
  exports.useDialogFocus({ current: dialog });
  return { document, previous, children, dialog, cleanup, keydown };
}

test("the shared trap skips hidden, disabled, inert, and negative-tabindex controls", () => {
  const view = mount([{ hidden: true, initial: true }, { disabled: true, initial: true }, { tabIndex: -2 }, { inert: true }, {}, {}]);
  const first = view.children[4], last = view.children[5];
  assert.equal(view.document.activeElement, first);
  let prevented = 0;
  view.keydown({ key: "Tab", shiftKey: true, preventDefault: () => prevented++ });
  assert.equal(view.document.activeElement, last);
  view.keydown({ key: "Tab", shiftKey: false, preventDefault: () => prevented++ });
  assert.equal(view.document.activeElement, first);
  assert.equal(prevented, 2);
  view.cleanup();
  assert.equal(view.document.activeElement, view.previous);
});

test("an empty dialog retains Tab focus and leaves consumed widget keys alone", () => {
  const view = mount([]);
  assert.equal(view.document.activeElement, view.dialog);
  let prevented = 0;
  view.keydown({ key: "Tab", preventDefault: () => prevented++ });
  assert.equal(prevented, 1);
  view.document.activeElement = view.previous;
  view.keydown({ key: "Tab", defaultPrevented: true, preventDefault: () => assert.fail("widget already owns Tab") });
  assert.equal(view.document.activeElement, view.previous);
  view.keydown({ key: "Escape", preventDefault: () => assert.fail("Escape belongs to its hotkey scope") });
});
