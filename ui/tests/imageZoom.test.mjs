import assert from "node:assert/strict";
import test from "node:test";
import { imageWheelZoom } from "../src/imageZoom.ts";

test("wheel zoom supports both directions, delta units, and the viewer limits", () => {
  assert(imageWheelZoom(1, -100, 0, 600) > 1);
  assert(imageWheelZoom(1, 100, 0, 600) < 1);
  assert.equal(imageWheelZoom(1, 1, 1, 600), imageWheelZoom(1, 16, 0, 600));
  assert.equal(imageWheelZoom(1, 1, 2, 600), imageWheelZoom(1, 600, 0, 600));
  assert.equal(imageWheelZoom(4, -100, 0, 600), 4);
  assert.equal(imageWheelZoom(0.25, 100, 0, 600), 0.25);
});
