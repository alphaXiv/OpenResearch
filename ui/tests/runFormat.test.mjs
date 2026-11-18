import assert from "node:assert/strict";
import test from "node:test";
import { setLocale } from "../src/locale.ts";
import { autoDir, ltr } from "../src/i18n.ts";
import { m } from "../src/paraglide/messages.js";
import { fillSentence, fmtSignal } from "../src/runMonitor/format.ts";

const bare = (text) => text.replace(/[⁦-⁩]/g, "");
const signals = { val_loss: { label: "Val loss", digits: 2, better: "lower" }, acc: { label: "Accuracy", digits: 1, unit: "%" }, elo: { label: "Elo", digits: 0, sign: true, better: "higher" } };

test("a sentence fills its signals, comparisons and counts and keeps its bold figure", () => {
  const bindings = { val_loss: 2.3147, "val_loss.vs": { d: -0.112, mag: 0.11, good: true, flat: false, parent: 1 }, 'count:"ERR 429"': 1234, elo: 832 };
  assert.equal(
    bare(fillSentence('Val loss **{val_loss}**, {val_loss.vs}; {count:"ERR 429"} rate limits; Elo {elo}.', bindings, signals)),
    "Val loss **2.31**, 0.11 better than the parent; 1,234 rate limits; Elo +832.",
  );
  // A comparison reads whole, so a sentence in any language need not finish it.
  assert.equal(bare(fillSentence("{val_loss.vs}", { "val_loss.vs": { d: 0.2, mag: 0.2, good: false, flat: false, parent: 1 } }, signals)), "0.20 worse than the parent");
  assert.equal(bare(fillSentence("{val_loss.vs}", { "val_loss.vs": { d: 0.001, mag: 0, good: false, flat: true, parent: 1 } }, signals)), "level with the parent");
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

test("watcher labels and values each keep their direction", (t) => {
  t.after(() => setLocale("en"));
  setLocale("ar");
  const label = "الخسارة";
  const specs = { val_loss: { ...signals.val_loss, label } };
  const comparison = { d: -0.11, mag: 0.11, good: true, flat: false, parent: 2.61 };
  const delta = ltr(fmtSignal(specs.val_loss, 0.11));
  const value = ltr(fmtSignal(specs.val_loss, 2.5));
  assert.equal(
    fillSentence(`${label} **{val_loss}** at this point. {val_loss.vs}.`, { val_loss: 2.5, "val_loss.vs": comparison }, specs),
    `${autoDir(label)} **${value}** at this point. ${autoDir(m.run_vs_token_better({ delta }))}.`,
  );
});

test("a comparison that no longer has a parent value drops its trailing clause", () => {
  const template = "Val loss **{val_loss}**, {val_loss.vs}.";
  assert.equal(bare(fillSentence(template, { val_loss: 2.5, "val_loss.vs": null }, signals)), "Val loss **2.50**.");
  assert.equal(bare(fillSentence(template, { val_loss: 2.5 }, signals)), "Val loss **2.50**.");
  assert.match(bare(fillSentence(template, { val_loss: 2.5, "val_loss.vs": { flat: true } }, signals)), /level with the parent/);
});

test("the output line count updates its grammar and uses the app locale", (t) => {
  t.after(() => setLocale("en"));
  const template = '**{count:"."}** lines of output so far.';
  const line = (count) => bare(fillSentence(template, { 'count:"."': count }, {}));
  assert.equal(line(1), "**1** line of output so far.");
  assert.equal(line(2), "**2** lines of output so far.");
  setLocale("es");
  assert.equal(line(1), "Hasta ahora, **1** línea de salida.");
  assert.equal(line(2), "Hasta ahora, **2** líneas de salida.");
});
