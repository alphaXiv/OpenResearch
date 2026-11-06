import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { getHotkeyManager, HotkeyManager, matchesKeyboardEvent } from "@tanstack/react-hotkeys";

import { appMenuFor } from "../src/appMenu.ts";
import { appHotkeys } from "../src/hotkeys.ts";

function keydown(init) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  // happy-dom reports AltGraph whenever Alt is held; browsers only for the AltGr key.
  event.getModifierState = (key) => key !== "AltGraph" && KeyboardEvent.prototype.getModifierState.call(event, key);
  return event;
}

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
  document.dispatchEvent(keydown({ key: "Escape" }));
  picker();
  document.dispatchEvent(keydown({ key: "Escape" }));
  dialog();
  document.dispatchEvent(keydown({ key: "Escape" }));
  assert.deepEqual(called, ["picker", "dialog", "stop"]);
});

test("a suspended Close Tab still prevents the platform's default close", () => {
  const manager = getHotkeyManager();
  const spec = appHotkeys["tab.close"];
  manager.register(spec.hotkey, () => assert.fail("dialog owns the keyboard"), { ...spec.options, platform: "windows" });
  const release = manager.activateScope("dialog", { exclusive: true });
  const event = keydown({ key: "w", ctrlKey: true });
  document.dispatchEvent(event);
  assert.equal(event.defaultPrevented, true);
  release();
});

test("new shortcuts match the intended platform and exact modifiers", () => {
  for (const platform of ["mac", "windows", "linux"]) {
    const mod = platform === "mac" ? { metaKey: true } : { ctrlKey: true };
    assert.equal(matchesKeyboardEvent(keydown({ key: ",", ...mod }), appHotkeys["settings.open"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(keydown({ key: "c", altKey: true, ...mod }), appHotkeys["compute.open"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(keydown({ key: "c", ...mod }), appHotkeys["compute.open"].hotkey, platform), false);
    assert.equal(matchesKeyboardEvent(keydown({ key: "b", ...mod }), appHotkeys["sidebar.toggle"].hotkey, platform), true);
    for (const [id, key] of [["experiments.open", "e"], ["files.open", "f"], ["artifacts.open", "a"], ["terminal.open", "t"]]) {
      assert.equal(matchesKeyboardEvent(keydown({ key, shiftKey: true, ...mod }), appHotkeys[id].hotkey, platform), true);
      assert.equal(matchesKeyboardEvent(keydown({ key, ...mod }), appHotkeys[id].hotkey, platform), false);
    }
    assert.equal(matchesKeyboardEvent(keydown({ key: "ArrowRight", altKey: true, ...mod }), appHotkeys["tab.nextArrow"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(keydown({ key: "Tab", ctrlKey: true }), appHotkeys["tab.next"].hotkey, platform), true);
    assert.equal(matchesKeyboardEvent(keydown({ key: "Tab", ctrlKey: true, shiftKey: true }), appHotkeys["tab.previous"].hotkey, platform), true);
    for (let number = 1; number <= 9; number++) {
      assert.equal(matchesKeyboardEvent(keydown({ key: String(number), ...mod }), appHotkeys[`tab.jump.${number}`].hotkey, platform), true);
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
