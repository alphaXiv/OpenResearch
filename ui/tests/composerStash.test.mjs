import assert from "node:assert/strict";
import test from "node:test";

import { attachmentToken, citedAttachments, composerStashContent, EMPTY_COMPOSER_STASH, createComposerState, withoutAttachmentTokens } from "../src/composerStash.ts";

test("an empty composer leaves nothing behind", () => {
  const live = { ...EMPTY_COMPOSER_STASH };
  assert.equal(composerStashContent(live, null), null);
});

test("an untouched demo prefill leaves nothing behind", () => {
  const live = { ...EMPTY_COMPOSER_STASH, draft: "Run the Muon matrix LR 2× probe experiment" };
  assert.equal(composerStashContent(live, live.draft), null);
});

test("a prefill edited by the user is stashed as their draft", () => {
  const prefill = "Run the Muon matrix LR 2× probe experiment";
  const live = { ...EMPTY_COMPOSER_STASH, draft: `${prefill} but on GPU` };
  assert.equal(composerStashContent(live, prefill)?.draft, live.draft);
  // An attachment paired with the untouched prefill is still user content.
  const withFile = { ...EMPTY_COMPOSER_STASH, draft: prefill, attachments: [{ dataUrl: "data:,a" }] };
  assert.ok(composerStashContent(withFile, prefill));
});

test("stashed annotations drop their transcript DOM ranges", () => {
  const live = {
    ...EMPTY_COMPOSER_STASH,
    draft: "",
    attachments: [],
    annotations: [{ id: "a1", text: "selected", range: { fake: "dom-range" } }],
  };
  const stashed = composerStashContent(live, null);
  assert.deepEqual(stashed.annotations, [{ id: "a1", text: "selected" }]);
});

test("an unconfirmed send remains stashed even if its draft is cleared", () => {
  const live = { ...EMPTY_COMPOSER_STASH, unconfirmedSend: "network" };
  assert.deepEqual(composerStashContent(live, null), live);
  assert.equal(composerStashContent({ ...live, unconfirmedSend: null }, null), null);
});

test("machines with cloned project and chat IDs keep their prompt seeds and failed drafts separate", () => {
  const alpha = createComposerState(), beta = createComposerState();
  alpha.seedComposer("same-project", "Reproduce Alpha");
  beta.seedComposer("same-project", "Reproduce Beta");
  alpha.composerStashes.set("same-chat", { ...EMPTY_COMPOSER_STASH, draft: "Alpha draft" });
  assert.equal(alpha.composerSeed("same-project"), "Reproduce Alpha");
  assert.equal(beta.composerSeed("same-project"), "Reproduce Beta");
  assert.equal(beta.composerStashes.has("same-chat"), false);
  alpha.forgetComposerSeed("same-project");
  assert.equal(beta.composerSeed("same-project"), "Reproduce Beta");
});

test("a draft sends the attachments it cites, renumbered in order of mention", () => {
  const image = { id: 7, dataUrl: "data:,a", mediaType: "image/png", size: 1 };
  const pdf = { id: 3, dataUrl: "data:,b", mediaType: "application/pdf", size: 1 };
  const deleted = { id: 9, dataUrl: "data:,c", mediaType: "image/png", size: 1 };
  assert.deepEqual(
    citedAttachments("see [PDF #3] then [Image #7], again [PDF #3], and [Image #42]", [image, pdf, deleted]),
    { text: "see [PDF #1] then [Image #2], again [PDF #1], and [Image #42]", attachments: [pdf, image] },
  );
});

test("any other file is cited as a file and renumbered with the rest", () => {
  const svg = { id: 4, dataUrl: "data:,d", mediaType: "image/svg+xml", size: 1 };
  const notes = { id: 5, dataUrl: "data:,e", mediaType: "application/octet-stream", size: 1 };
  assert.equal(attachmentToken(svg), "[File #4]");
  assert.deepEqual(
    citedAttachments("[File #5] and [File #4]", [svg, notes]),
    { text: "[File #1] and [File #2]", attachments: [notes, svg] },
  );
});

test("a sent message hides its attachment tokens but keeps the user's spacing", () => {
  assert.equal(withoutAttachmentTokens("[Image #1] [File #2] What does this show?"), "What does this show?");
  assert.equal(withoutAttachmentTokens("see [Image #1] here"), "see here");
  assert.equal(withoutAttachmentTokens("[PDF #1]\n```\ndef f():\n    return  1\n```"), "```\ndef f():\n    return  1\n```");
  assert.equal(withoutAttachmentTokens("[File #1]\n    indented"), "    indented");
});

test("a seeded composer carries the text without the attachments it cites", () => {
  const state = createComposerState();
  state.seedComposer("p", "Compare [Image #1] with the paper");
  assert.equal(state.composerSeed("p"), "Compare  with the paper");
  state.seedComposer("q", "[PDF #2]");
  assert.equal(state.composerSeed("q"), null);
});
