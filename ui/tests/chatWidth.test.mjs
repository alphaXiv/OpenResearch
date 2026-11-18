import assert from "node:assert/strict";
import test from "node:test";

import { CHAT_WIDTHS, parseChatWidth, setChatWidth } from "../src/chatWidth.ts";

test("unknown chat widths fall back to the balanced measure", () => {
  assert.equal(parseChatWidth(null), "balanced");
  assert.equal(parseChatWidth("narrow"), "balanced");
  assert.equal(parseChatWidth("wide "), "balanced");
  for (const width of CHAT_WIDTHS) assert.equal(parseChatWidth(width), width);
});

test("a chosen chat width reaches the document and storage", () => {
  setChatWidth("full");
  assert.equal(document.documentElement.dataset.chatWidth, "full");
  assert.equal(localStorage.getItem("orx:chat-width"), "full");

  setChatWidth("balanced");
  assert.equal(document.documentElement.dataset.chatWidth, "balanced");
  assert.equal(localStorage.getItem("orx:chat-width"), "balanced");
});

test("a blocked storage write still applies the width for this session", () => {
  const original = localStorage.setItem.bind(localStorage);
  localStorage.setItem = () => {
    throw new Error("blocked");
  };
  try {
    setChatWidth("wide");
    assert.equal(document.documentElement.dataset.chatWidth, "wide");
  } finally {
    localStorage.setItem = original;
  }
  setChatWidth("balanced");
});
