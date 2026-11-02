import assert from "node:assert/strict";
import test from "node:test";

import { composerStashContent, EMPTY_COMPOSER_STASH } from "../src/composerStash.ts";

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
