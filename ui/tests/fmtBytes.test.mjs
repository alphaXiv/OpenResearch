import assert from "node:assert/strict";
import test from "node:test";
import { fmtBytes, fmtMemory } from "../src/i18n.ts";
import { setLocale } from "../src/locale.ts";

const KiB = 1024;
const MiB = 1024 ** 2;
const GiB = 1024 ** 3;

test("byte sizes read in IEC units to three significant figures", () => {
  assert.equal(fmtBytes(0), "0 B");
  assert.equal(fmtBytes(512), "512 B");
  assert.equal(fmtBytes(1023), "1,023 B");
  assert.equal(fmtBytes(1536), "1.5 KiB");
  assert.equal(fmtBytes(512 * KiB), "512 KiB");
  assert.equal(fmtBytes(30 * MiB), "30 MiB");
  assert.equal(fmtBytes(12.34 * MiB), "12.3 MiB");
  assert.equal(fmtBytes(16 * GiB), "16 GiB");
  assert.equal(fmtBytes(3.971 * GiB), "3.97 GiB");
  assert.equal(fmtBytes(2048 * 1024 ** 4), "2,048 TiB");
});

test("a size that would round to 1,024 of a unit moves up to the next one", () => {
  assert.equal(fmtBytes(1023.9 * KiB), "1 MiB");
  assert.equal(fmtBytes(1023.4 * KiB), "1,023 KiB");
});

test("byte sizes follow the locale's digits and unit names", (t) => {
  t.after(() => setLocale("en"));

  setLocale("ru");
  assert.equal(fmtBytes(3.97 * GiB), "3,97 ГиБ");
  setLocale("fa");
  assert.equal(fmtBytes(512), "۵۱۲ بایت");
});

test("installed memory reads like the spec sheet", () => {
  assert.equal(fmtMemory(48 * GiB), "48 GB");
  assert.equal(fmtMemory(62.7 * GiB), "63 GB");
});
