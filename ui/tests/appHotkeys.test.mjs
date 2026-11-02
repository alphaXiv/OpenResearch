import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { afterEach, test } from "node:test";
import ts from "typescript";
import { getHotkeyManager, HotkeyManager, matchesKeyboardEvent } from "@tanstack/react-hotkeys";

const require = createRequire(import.meta.url);
class KeyboardEvent extends Event {
  constructor(type, options = {}) {
    const { bubbles = true, cancelable = true, ...keys } = options;
    super(type, { bubbles, cancelable });
    Object.assign(this, { key: "", code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, isComposing: false }, keys);
  }
  getModifierState() { return false; }
}
globalThis.KeyboardEvent = KeyboardEvent;
globalThis.window = new EventTarget();
globalThis.document = Object.assign(new EventTarget(), { nodeType: 9, activeElement: null });
const en = JSON.parse(readFileSync(new URL("../messages/en.json", import.meta.url), "utf8"));
const m = new Proxy({}, { get: (_, key) => (inputs = {}) => en[key].replace(/\{(\w+)\}/g, (_, name) => inputs[name]) });

function load(path, mocks = {}) {
  const { outputText } = ts.transpileModule(readFileSync(new URL(path, import.meta.url), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const exports = {};
  new Function("require", "exports", outputText)((id) => mocks[id] ?? require(id), exports);
  return exports;
}
const hotkeys = load("../src/hotkeys.ts", { "./paraglide/messages.js": { m } });
const { appHotkeys } = hotkeys;
const { appMenuFor } = load("../src/appMenu.ts", { "./hotkeys": hotkeys, "./paraglide/messages.js": { m } });

afterEach(() => HotkeyManager.resetInstance());

test("native menu commands respect disabled registrations and exclusive scopes", () => {
  const manager = getHotkeyManager();
  const called = [];
  manager.register("Mod+W", () => called.push("disabled"), { appHotkeyId: "tab.close", enabled: false, platform: "mac" });
  const close = manager.register("Mod+W", () => called.push("close"), { appHotkeyId: "tab.close", platform: "mac", conflictBehavior: "allow" });
  const run = () => window.dispatchEvent(new CustomEvent("orx:command", { detail: "tab.close" }));
  run();
  assert.deepEqual(called, ["close"]);
  const release = manager.activateScope("picker", { exclusive: true });
  run();
  assert.deepEqual(called, ["close"]);
  release();
  close.setOptions({ enabled: false });
  run();
  assert.deepEqual(called, ["close"]);
});

test("Escape dismisses the innermost picker, then the dialog, then stops the turn", () => {
  const manager = getHotkeyManager();
  const called = [];
  manager.register("Escape", () => called.push("stop"));
  const dialog = manager.activateScope("dialog", { exclusive: true });
  manager.register("Escape", () => called.push("dialog"), { scope: "dialog" });
  const picker = manager.activateScope("picker", { exclusive: true, parent: "dialog" });
  manager.register("Escape", () => called.push("picker"), { scope: "picker" });
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  picker();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  dialog();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  assert.deepEqual(called, ["picker", "dialog", "stop"]);
});

test("a suspended Close Tab still prevents the platform's default close", () => {
  const manager = getHotkeyManager();
  const spec = appHotkeys["tab.close"];
  manager.register(spec.hotkey, () => assert.fail("dialog owns the keyboard"), { ...spec.options, platform: "windows" });
  const release = manager.activateScope("dialog", { exclusive: true });
  const event = new KeyboardEvent("keydown", { key: "w", ctrlKey: true });
  document.dispatchEvent(event);
  assert.equal(event.defaultPrevented, true);
  release();
});

test("new shortcuts match the intended platform and exact modifiers", () => {
  for (const platform of ["mac", "windows", "linux"]) {
    const mod = platform === "mac" ? { metaKey: true } : { ctrlKey: true };
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: ",", ...mod }), appHotkeys["settings.open"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "c", altKey: true, ...mod }), appHotkeys["compute.open"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "c", ...mod }), appHotkeys["compute.open"].hotkey, platform), false);
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "b", ...mod }), appHotkeys["sidebar.toggle"].hotkey, platform), true);
    for (const [id, key] of [["experiments.open", "e"], ["files.open", "f"], ["artifacts.open", "a"], ["terminal.open", "t"]]) {
      assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key, shiftKey: true, ...mod }), appHotkeys[id].hotkey, platform), true);
      assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key, ...mod }), appHotkeys[id].hotkey, platform), false);
    }
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "ArrowRight", altKey: true, ...mod }), appHotkeys["tab.nextArrow"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true }), appHotkeys["tab.next"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true, shiftKey: true }), appHotkeys["tab.previous"].hotkey, platform), true);
    for (let number = 1; number <= 9; number++) {
      assert.equal(matchesKeyboardEvent(new KeyboardEvent("keydown", { key: String(number), ...mod }), appHotkeys[`tab.jump.${number}`].hotkey, platform), true);
    }
  }
});

test("macOS keeps native window commands; Windows offers the same page actions", () => {
  const commands = (platform) => appMenuFor(platform).flatMap((section) => section.items).filter((item) => item.kind === "command").map((item) => item.id);
  assert.ok(commands("mac").includes("window.close"));
  assert.equal(commands("windows").includes("window.close"), false);
  for (const id of ["settings.open", "compute.open", "experiments.open", "files.open", "artifacts.open", "terminal.open", "sidebar.toggle", "tab.next", "tab.previous", "tab.jump.9"]) {
    assert.ok(commands("mac").includes(id));
    assert.ok(commands("windows").includes(id));
  }
  assert.equal(commands("mac").filter((id) => id === "settings.open").length, 1);
  assert.equal(commands("windows").filter((id) => id === "settings.open").length, 1);
  assert.equal(commands("mac").some((id) => /reload/i.test(id)), false);
});
