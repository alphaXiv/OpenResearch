import assert from "node:assert/strict";
import test from "node:test";
import { navigateKeyboardList, navigateKeyboardRow } from "../src/keyboardNavigation.ts";

let focused;
class Element {
  constructor({ item = false, horizontal = false, path, expanded, hidden = false, disabled = false } = {}) {
    this.item = item;
    this.horizontal = horizontal;
    this.dataset = path ? { keyboardPath: path } : {};
    this.expanded = expanded;
    this.hidden = hidden;
    this.disabled = disabled;
  }
  hasAttribute(name) { return name === "data-keyboard-item" ? this.item : name === "data-keyboard-horizontal-item" && this.horizontal; }
  getAttribute(name) { return name === "aria-expanded" ? this.expanded ?? null : null; }
  getClientRects() { return this.hidden ? [] : [{}]; }
  matches() { return this.disabled; }
  closest() { return this.list; }
  focus() { focused = this; }
  click() { this.expanded = this.expanded === "true" ? "false" : "true"; }
  querySelector() { return this.list; }
  querySelectorAll() { return this.items; }
}
globalThis.HTMLElement = Element;
globalThis.getComputedStyle = () => ({ direction: "ltr" });

function key(root, target, value, overrides = {}) {
  const event = { currentTarget: root, target, key: value, nativeEvent: { isComposing: false }, defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; }, ...overrides };
  return event;
}

test("panel arrows enter and navigate visible list rows without wrapping", () => {
  const root = new Element();
  const list = new Element();
  const first = new Element({ item: true });
  const hidden = new Element({ item: true, hidden: true });
  const disabled = new Element({ item: true, disabled: true });
  const last = new Element({ item: true });
  root.list = list;
  list.items = [first, hidden, disabled, last];
  for (const item of list.items) item.list = list;
  navigateKeyboardList(key(root, root, "ArrowDown"));
  assert.equal(focused, first);
  navigateKeyboardList(key(root, first, "ArrowDown"));
  assert.equal(focused, last);
  navigateKeyboardList(key(root, last, "ArrowDown"));
  assert.equal(focused, last);
  navigateKeyboardList(key(root, last, "Home"));
  assert.equal(focused, first);
  navigateKeyboardList(key(root, first, "End"));
  assert.equal(focused, last);
});

test("tree arrows expand, enter children, return to parents and collapse", () => {
  const root = new Element();
  const list = new Element();
  const parent = new Element({ item: true, path: "dir", expanded: "false" });
  const child = new Element({ item: true, path: "dir/file" });
  root.list = list;
  list.items = [parent, child];
  parent.list = child.list = list;
  navigateKeyboardList(key(root, parent, "ArrowRight"));
  assert.equal(parent.expanded, "true");
  navigateKeyboardList(key(root, parent, "ArrowRight"));
  assert.equal(focused, child);
  navigateKeyboardList(key(root, child, "ArrowLeft"));
  assert.equal(focused, parent);
  navigateKeyboardList(key(root, parent, "ArrowLeft"));
  assert.equal(parent.expanded, "false");
});

test("typing controls and modified arrows keep their own keyboard behavior", () => {
  const root = new Element();
  const input = new Element();
  const event = key(root, input, "ArrowDown");
  navigateKeyboardList(event);
  assert.equal(event.defaultPrevented, false);
  const modified = key(root, root, "ArrowDown", { ctrlKey: true });
  navigateKeyboardList(modified);
  assert.equal(modified.defaultPrevented, false);
});

test("horizontal groups move focus, wrap, and respect right-to-left layout", () => {
  const root = new Element();
  const first = new Element({ horizontal: true });
  const last = new Element({ horizontal: true });
  root.items = [first, last];
  navigateKeyboardRow(key(root, first, "ArrowRight"));
  assert.equal(focused, last);
  navigateKeyboardRow(key(root, last, "ArrowRight"));
  assert.equal(focused, first);
  globalThis.getComputedStyle = () => ({ direction: "rtl" });
  navigateKeyboardRow(key(root, first, "ArrowRight"));
  assert.equal(focused, last);
  globalThis.getComputedStyle = () => ({ direction: "ltr" });
});
