import assert from "node:assert/strict";
import test from "node:test";
import { JSDOM } from "jsdom";
import { findTextMatches, scrollTranscriptSearchRangeIntoView, transcriptSearchRanges } from "../src/chatSearch.ts";

function searchDocument(t, html) {
  const { window } = new JSDOM(html);
  for (const name of ["Element", "NodeFilter", "getComputedStyle"]) {
    const previous = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { configurable: true, value: window[name] });
    t.after(() => {
      if (previous) Object.defineProperty(globalThis, name, previous);
      else delete globalThis[name];
    });
  }
  t.after(() => window.close());
  return window.document;
}

test("chat find treats regex punctuation literally", () => {
  const text = "Use a.*[b](c)? + $ ^ {x} | \\ here";
  for (const query of [".*", "[b]", "(c)?", "+", "$", "^", "{x}", "|", "\\"]) {
    const start = text.indexOf(query);
    assert.deepEqual(findTextMatches(text, query), [{ start, end: start + query.length }]);
  }
});

test("case-insensitive Unicode matching preserves UTF-16 range offsets", () => {
  assert.deepEqual(findTextMatches("🔍 Привет привет ПРИВЕТ", "привет"), [
    { start: 3, end: 9 }, { start: 10, end: 16 }, { start: 17, end: 23 },
  ]);
  assert.deepEqual(findTextMatches("İ foo FOO", "foo"), [{ start: 2, end: 5 }, { start: 6, end: 9 }]);
  assert.deepEqual(findTextMatches("你好世界你好", "你好"), [{ start: 0, end: 2 }, { start: 4, end: 6 }]);
});

test("empty and missing queries have no matches, repeated matches do not overlap", () => {
  assert.deepEqual(findTextMatches("hello", ""), []);
  assert.deepEqual(findTextMatches("hello", "world"), []);
  assert.deepEqual(findTextMatches("aaaa", "aa"), [{ start: 0, end: 2 }, { start: 2, end: 4 }]);
});

test("closed details search only the first summary and refresh after opening and closing", (t) => {
  const document = searchDocument(t, `<main><div data-message-id="message">
    <details>hidden needle before summary<summary>Visible <strong>needle</strong></summary>
      hidden needle after summary<p>Plan <span>needle</span></p><summary>Second needle</summary>
    </details><p>Outside needle</p>
  </div></main>`);
  const root = document.querySelector("main");
  const details = document.querySelector("details");
  assert.equal(transcriptSearchRanges(root, "needle").length, 2);
  details.open = true;
  assert.equal(transcriptSearchRanges(root, "needle").length, 6);
  details.open = false;
  assert.equal(transcriptSearchRanges(root, "needle").length, 2);
});

test("nested details honor each disclosure's visibility", (t) => {
  const document = searchDocument(t, `<main><div data-message-id="message">
    <details id="outer"><summary>Outer needle</summary>
      <details id="inner"><summary>Inner needle</summary><p>Hidden needle</p></details>
    </details>
  </div></main>`);
  const root = document.querySelector("main");
  const outer = document.querySelector("#outer");
  const inner = document.querySelector("#inner");
  assert.equal(transcriptSearchRanges(root, "needle").length, 1);
  outer.open = true;
  assert.equal(transcriptSearchRanges(root, "needle").length, 2);
  inner.open = true;
  assert.equal(transcriptSearchRanges(root, "needle").length, 3);
  outer.open = false;
  assert.equal(transcriptSearchRanges(root, "needle").length, 1);
});

test("DOM ranges join inline formatting without crossing paragraph or disclosure boundaries", (t) => {
  const document = searchDocument(t, `<main><div data-message-id="message">
    <p>Find <strong>across</strong> <code>inline</code> markup</p>
    <p>Separate</p><p>paragraph</p>
    <details><summary>Summary</summary><p>hidden</p></details>after
    <div hidden>hidden needle</div><div inert>inert needle</div><div aria-hidden="true">aria needle</div>
  </div></main>`);
  const root = document.querySelector("main");
  const ranges = transcriptSearchRanges(root, "across inline markup");
  assert.equal(ranges.length, 1);
  assert.equal(ranges[0].toString(), "across inline markup");
  assert.deepEqual(transcriptSearchRanges(root, "Separateparagraph"), []);
  assert.deepEqual(transcriptSearchRanges(root, "Summaryafter"), []);
  assert.deepEqual(transcriptSearchRanges(root, "needle"), []);
});

function metrics(element, values) {
  for (const [name, value] of Object.entries(values)) Object.defineProperty(element, name, { value });
}

const rect = (top, left, height, width) => ({ top, left, height, width, bottom: top + height, right: left + width });

test("match navigation reveals nested vertical output and horizontal code before positioning the chat", (t) => {
  const document = searchDocument(t, `<main id="chat"><div id="output" style="overflow-y:auto">
    <pre id="code" style="overflow-x:auto;overflow-y:auto">needle</pre>
  </div></main>`);
  const chat = document.querySelector("#chat");
  const output = document.querySelector("#output");
  const code = document.querySelector("#code");
  metrics(chat, { clientHeight: 400, scrollHeight: 3000 });
  metrics(output, { clientHeight: 200, scrollHeight: 1200, clientWidth: 200, scrollWidth: 200 });
  metrics(code, { clientHeight: 100, scrollHeight: 1000, clientWidth: 100, scrollWidth: 800 });
  chat.getBoundingClientRect = () => rect(20, 0, 400, 300);
  output.getBoundingClientRect = () => rect(20 + 1000 - chat.scrollTop, 0, 200, 200);
  code.getBoundingClientRect = () => rect(output.getBoundingClientRect().top + 500 - output.scrollTop, 0, 100, 100);
  const range = transcriptSearchRanges(chat, "needle")[0];
  range.getBoundingClientRect = () => rect(code.getBoundingClientRect().top + 700 - code.scrollTop, 600 - code.scrollLeft, 20, 40);

  scrollTranscriptSearchRangeIntoView(range, chat);

  const match = range.getBoundingClientRect();
  for (const element of [code, output, chat]) {
    assert.ok(element.scrollTop > 0, `${element.id} scrolled vertically`);
    const bounds = element.getBoundingClientRect();
    assert.ok(match.top >= bounds.top && match.bottom <= bounds.bottom, `match is visible in ${element.id}`);
  }
  assert.ok(code.scrollLeft > 0);
  assert.ok(match.left >= 0 && match.right <= code.clientWidth);
});

test("match navigation leaves non-scrollable ancestors untouched", (t) => {
  const document = searchDocument(t, '<main><div style="overflow:hidden">needle</div></main>');
  const chat = document.querySelector("main");
  const container = document.querySelector("div");
  metrics(chat, { clientHeight: 400 });
  metrics(container, { clientHeight: 100, scrollHeight: 900, clientWidth: 100, scrollWidth: 900 });
  chat.getBoundingClientRect = () => rect(20, 0, 400, 300);
  container.getBoundingClientRect = () => rect(20 + 1000 - chat.scrollTop, 0, 100, 100);
  const range = transcriptSearchRanges(chat, "needle")[0];
  range.getBoundingClientRect = () => rect(20 + 1000 - chat.scrollTop, 0, 20, 40);

  scrollTranscriptSearchRangeIntoView(range, chat);

  assert.equal(container.scrollTop, 0);
  assert.equal(container.scrollLeft, 0);
  assert.ok(chat.scrollTop > 0);
  const match = range.getBoundingClientRect();
  assert.ok(match.top >= 20 && match.bottom <= 420);
});
