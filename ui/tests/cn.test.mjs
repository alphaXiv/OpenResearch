import assert from "node:assert/strict";
import test from "node:test";
import { cn } from "../src/components/ui/cn.ts";

test("compact menu font size survives danger colors and supports size overrides", () => {
  assert.equal(cn("text-menu", "text-accent-red"), "text-menu text-accent-red");
  assert.equal(cn("text-menu", "text-sm"), "text-sm");
});
