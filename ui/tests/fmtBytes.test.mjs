import assert from "node:assert/strict";
import test from "node:test";
import { fmtBytes } from "../src/api.ts";

test("fmtBytes rolls over to the next unit instead of rounding to '1024.0'", () => {
  // 1023.98 MB: rounding to one decimal place must not land on "1024.0 MB" —
  // it should cross the unit boundary and read "1.0 GB" instead.
  assert.equal(fmtBytes(1073730000), "1.0 GB");
  assert.equal(fmtBytes(1073741823), "1.0 GB"); // one byte short of a full GB
});

test("fmtBytes keeps formatting plain sizes away from a unit boundary", () => {
  assert.equal(fmtBytes(512), "512 B");
  assert.equal(fmtBytes(2048), "2.0 KB");
  assert.equal(fmtBytes(5 * 1024 * 1024), "5.0 MB");
  assert.equal(fmtBytes(1048000000), "999.5 MB");
});
