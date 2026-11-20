import { EditorView } from "@codemirror/view";
import assert from "node:assert/strict";
import test from "node:test";
import { act, createElement } from "react";
import { ComposerEditor } from "../src/components/ComposerEditor.tsx";
import { render } from "./render.mjs";

const PNG = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
const attachments = new Map([
  [1, { id: 1, dataUrl: PNG, mediaType: "image/png", name: "chart.png", size: 70 }],
  [2, { id: 2, dataUrl: "data:text/markdown;base64,", mediaType: "text/markdown", name: "notes.md", size: 0 }],
]);
const chipSource = {
  isCommand: () => false, skill: () => undefined, message: () => undefined, chat: () => undefined, isPath: () => false,
  attachment: (id) => attachments.get(id),
};
const ignore = () => {};

async function composer(value) {
  const changes = [];
  const view = await render(createElement(ComposerEditor, {
    value, onChange: (next) => changes.push(next), onKeyDown: ignore, onSubmit: ignore, onFiles: ignore, onPaste: ignore,
    placeholder: "Message", plain: false, chipSource, projectId: "p", draftKey: "draft", resizeKey: "composer-height",
  }));
  const field = view.container.querySelector(".cm-content");
  return {
    editor: EditorView.findFromDOM(field),
    chips: () => [...field.querySelectorAll("[data-chip]")].map((chip) => chip.dataset.chip),
    remove: (name) => act(async () => field.querySelector(`[aria-label$="${name}"]`).click()),
    changes,
  };
}

test("adjacent attachments stay shown wherever the caret goes", async () => {
  const chat = await composer("[Image #1][File #2]\nsecond line");
  await act(async () => chat.editor.dispatch({ selection: { anchor: chat.editor.state.doc.length } }));
  assert.deepEqual(chat.chips(), ["#1", "#2"]);
  await act(async () => chat.editor.dispatch({ selection: { anchor: 0 } }));
  assert.deepEqual(chat.chips(), ["#1", "#2"]);
});

test("an attachment's remove button takes only its own token out of the draft", async () => {
  const chat = await composer("look [Image #1] and [File #2] here");
  await chat.remove("chart.png");
  assert.equal(chat.editor.state.doc.toString(), "look and [File #2] here");
  assert.deepEqual(chat.chips(), ["#2"]);
});
