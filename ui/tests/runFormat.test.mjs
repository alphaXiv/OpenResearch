import assert from "node:assert/strict";
import test from "node:test";
import { setLocale } from "../src/locale.ts";
import { fillSentence, fmtSignal } from "../src/runMonitor/format.ts";

const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const signals = { val_loss: { label: "Val loss", digits: 2, better: "lower" }, acc: { label: "Accuracy", digits: 1, unit: "%" }, elo: { label: "Elo", digits: 0, sign: true, better: "higher" } };

test("a sentence fills its signals, comparisons and counts and keeps its bold figure", () => {
  const bindings = { val_loss: 2.3147, "val_loss.vs": -0.112, 'count:"ERR 429"': 1234, elo: 832 };
  assert.equal(
    bare(fillSentence('Val loss **{val_loss}**, {val_loss.vs} than the parent; {count:"ERR 429"} rate limits; Elo {elo}.', bindings, signals)),
    "Val loss **2.31**, 0.11 better than the parent; 1,234 rate limits; Elo +832.",
  );
  assert.equal(bare(fillSentence("{val_loss.vs}", { "val_loss.vs": 0.2 }, signals)), "0.20 worse");
  assert.equal(bare(fillSentence("{val_loss.vs}", { "val_loss.vs": 0.001 }, signals)), "level with");
});

test("a value the server can't give as a number reads as it is, and an unknown one as a dash", () => {
  assert.equal(bare(fillSentence("Loss is **{val_loss}**.", { val_loss: "NaN" }, signals)), "Loss is **NaN**.");
  assert.equal(fmtSignal(signals.val_loss, "Infinity"), "∞");
  assert.equal(bare(fillSentence("Loss {val_loss}, {val_loss.vs}, {missing}, {count:\"x\"}.", { val_loss: null }, signals)), "Loss —, —, —, —.");
});

test("numbers take the app's language", (t) => {
  t.after(() => setLocale("en"));
  setLocale("ar");
  assert.equal(bare(fillSentence("{val_loss}", { val_loss: 2.5 }, signals)), new Intl.NumberFormat("ar", { minimumFractionDigits: 2 }).format(2.5));
  setLocale("pl");
  assert.equal(fmtSignal(signals.acc, 52.6), "52,6%");
  assert.equal(bare(fillSentence('{count:"e"}', { 'count:"e"': 12000 }, signals)), new Intl.NumberFormat("pl").format(12000));
});
