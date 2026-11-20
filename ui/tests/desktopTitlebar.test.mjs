import assert from "node:assert/strict";
import test from "node:test";

test("the macOS app drags its window, without selecting text, from the top strip and from areas marked as titlebar", async () => {
  const messages = [];
  window.__ORX_MAC_TITLEBAR__ = true;
  window.ipc = { postMessage: (message) => messages.push(message) };
  await import("../src/desktopTitlebar.ts");

  document.body.innerHTML = `
    <div id="strip">brand</div>
    <div id="head" data-titlebar-drag><span id="label">Chats</span><button id="new">New chat</button></div>
    <div id="list">a chat</div>`;
  const press = (id, clientY) => {
    messages.length = 0;
    const target = document.getElementById(id);
    const selects = target.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, detail: 1, clientY }));
    window.dispatchEvent(new MouseEvent("mousemove", { buttons: 1 }));
    window.dispatchEvent(new MouseEvent("mouseup"));
    return [...messages, ...selects ? [] : ["no selection"]];
  };

  assert.deepEqual(press("strip", 20), ["titlebar:drag", "no selection"]);
  assert.deepEqual(press("label", 60), ["titlebar:drag", "no selection"]);
  assert.deepEqual(press("new", 60), []);
  assert.deepEqual(press("list", 60), []);
});
