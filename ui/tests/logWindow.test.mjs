import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { createLogFollower, dropPartialChar, isKept, lastWindow, LOG_WINDOW, LogPageUnavailable, parsePosition, shouldRecover, windowAt } from "../src/logWindow.ts";

const MiB = 1024 ** 2;
const GiB = 1024 ** 3;

test("windows stay inside the log", () => {
  const len = 10 * LOG_WINDOW;
  assert.deepEqual(windowAt(0, 300), { start: 0, end: 300 });
  assert.deepEqual(windowAt(-5, len), { start: 0, end: LOG_WINDOW });
  assert.deepEqual(windowAt(9.5 * LOG_WINDOW, len), { start: 9 * LOG_WINDOW, end: len });
  assert.deepEqual(lastWindow(len), { start: 9 * LOG_WINDOW, end: len });
  assert.deepEqual(lastWindow(300), { start: 0, end: 300 });
});

test("a span is kept only when one kept range covers it", () => {
  const kept = [[0, 64 * MiB], [4 * GiB - 256 * MiB, 4 * GiB]];
  assert.equal(isKept(kept, { start: 0, end: LOG_WINDOW }), true);
  assert.equal(isKept(kept, lastWindow(4 * GiB)), true);
  assert.equal(isKept(kept, { start: 2 * GiB, end: 2 * GiB + LOG_WINDOW }), false);
  assert.equal(isKept(kept, { start: 64 * MiB - 10, end: 64 * MiB + 10 }), false);
  assert.equal(isKept([], { start: 0, end: 10 }), false);
});

test("typed positions accept sizes, percents and offsets", () => {
  const len = 4 * GiB;
  assert.equal(parsePosition("50%", len), 2 * GiB);
  assert.equal(parsePosition("2.5 GB", len), 2.5 * GiB);
  assert.equal(parsePosition("2,5g", len), 2.5 * GiB);
  assert.equal(parsePosition("512 MiB", len), 512 * MiB);
  assert.equal(parsePosition("1024", len), 1024);
  assert.equal(parsePosition("9 TB", len), len);
  assert.equal(parsePosition("soon", len), null);
  assert.equal(parsePosition("", len), null);
});

test("only an incomplete UTF-8 prefix is dropped, never a whole line", () => {
  const bytes = (...values) => Uint8Array.from(values);
  assert.deepEqual(dropPartialChar(bytes(0x82, 0xac, 0x41)), bytes(0x41));
  assert.deepEqual(dropPartialChar(new TextEncoder().encode("line one\nline two\n")), new TextEncoder().encode("line one\nline two\n"));
  assert.deepEqual(dropPartialChar(bytes(0xe2, 0x82, 0xac)), bytes(0xe2, 0x82, 0xac));
});

test("recovery retries a page only when its failure predates the host's return", () => {
  const page = (epoch) => ({ host: "gpu-box", since: 1, contact: "unreachable", epoch });
  assert.equal(shouldRecover(page(0), true, 1), true);
  assert.equal(shouldRecover(page(1), true, 1), false);
  assert.equal(shouldRecover(page(0), false, 1), false);
  assert.equal(shouldRecover(null, true, 1), false);
});

const flush = () => new Promise((resolve) => setImmediate(resolve));
const b64 = (bytes) => Buffer.from(bytes).toString("base64");
const data = (start, bytes, extra = {}) => ({ kind: "data", ident: "file-a", start, data: b64(bytes), eof: false, len: null, kept: [], ...extra });
const text = (value) => new TextEncoder().encode(value);

function harness(respond) {
  const requests = [];
  const sink = { output: "", resets: 0, tops: 0, reset() { this.output = ""; this.resets += 1; }, write(value) { this.output += value; }, scrollToTop() { this.tops += 1; } };
  let view;
  let epoch = 0;
  const follower = createLogFollower({
    unstableMessage: "The log keeps changing. Reload to try again.",
    fetchPage: async (span, ident) => {
      requests.push({ ...span, ident });
      return respond(span, ident, requests.length);
    },
    sink,
    publish: (next) => { view = next; },
    epoch: () => epoch,
  });
  return { follower, requests, sink, view: () => view, setEpoch: (value) => { epoch = value; } };
}

describe("log follower", () => {
  test("a run with no log file yet waits for an event instead of re-reading", async () => {
    const log = harness(() => ({ kind: "reset", info: null }));
    log.follower.follow();
    await flush();
    await flush();
    assert.equal(log.requests.length, 1);
    assert.equal(log.view().loading, false);

    log.follower.onLog({ dataBase64: "", offset: 12 });
    await flush();
    assert.equal(log.requests.length, 2);
  });

  test("a replaced file resets once, then follows the new one", async () => {
    let current = "file-a";
    const log = harness((_, ident) => ident !== null && ident !== current
      ? { kind: "reset", info: { ident: current, len: 4 } }
      : data(0, text("new\n"), { ident: current, eof: true }));
    log.follower.follow();
    await flush();
    current = "file-b";
    log.follower.onLog({ dataBase64: "", offset: 10 });
    await flush();
    await flush();
    await flush();
    assert.deepEqual(log.requests.map((request) => request.ident), [null, "file-a", null]);
    assert.equal(log.sink.output, "new\n");
  });

  test("the ident is passed back verbatim", async () => {
    const opaque = "18446744073709551615:9007199254740993:abc";
    const log = harness((span) => data(span.start, span.start === 0 ? text("one\n") : new Uint8Array(), { ident: opaque }));
    log.follower.follow();
    await flush();
    log.follower.onLog({ dataBase64: "", offset: 9 });
    await flush();
    assert.equal(log.requests.at(-1).ident, opaque);
  });

  test("a shorter replacement replaces the shown output", async () => {
    let file = "a".repeat(100);
    const log = harness((span) => data(span.start, text(file.slice(span.start)), { eof: true }));
    log.follower.follow();
    await flush();
    assert.equal(log.sink.output, file);

    file = "b".repeat(10);
    log.follower.onLog({ dataBase64: "", offset: 10 });
    await flush();
    await flush();
    assert.equal(log.sink.output, file);
    assert.equal(log.view().len, 10);
  });

  test("a legacy generation change resyncs from the start", async () => {
    let file = "first file\n";
    const log = harness((span) => data(span.start, text(file.slice(span.start)), { eof: true }));
    log.follower.follow();
    await flush();
    log.follower.onLog({ dataBase64: "", offset: file.length, generation: "g1" });
    await flush();

    file = "second\n";
    log.follower.onLog({ dataBase64: b64(text("second\n")), offset: 0, generation: "g2" });
    await flush();
    await flush();
    assert.equal(log.sink.output, "second\n");
  });

  test("a reconnect catches a following view up on growth it missed", async () => {
    let file = "before\n";
    const log = harness((span) => data(span.start, text(file.slice(span.start))));
    log.follower.follow();
    await flush();
    file = "before\nduring the gap\n";
    log.follower.onReconnect();
    await flush();
    assert.equal(log.sink.output, file);
  });

  test("a failed read records the epoch it started under", async () => {
    let fail;
    const log = harness(() => new Promise((_, reject) => { fail = reject; }));
    log.follower.follow();
    await flush();
    log.setEpoch(1);
    fail(new LogPageUnavailable("gpu-box", 5, "unreachable"));
    await flush();
    assert.deepEqual(log.view().unavailable, { host: "gpu-box", since: 5, contact: "unreachable", epoch: 0 });
    assert.equal(shouldRecover(log.view().unavailable, true, 1), true);
  });

  test("a character split across unrelated windows does not join", async () => {
    const len = 3 * LOG_WINDOW;
    const log = harness((span) => {
      if (span.start === 0) return data(0, Uint8Array.from([0x41, 0xe2]), { len });
      return data(span.start, span.start === LOG_WINDOW ? Uint8Array.from([0x82, 0xac, 0x41]) : new Uint8Array(), { len });
    });
    log.follower.goTo(0);
    await flush();
    log.follower.goTo(LOG_WINDOW);
    await flush();
    assert.equal(log.sink.output, "A");
  });

  test("a character split across contiguous deltas decodes whole", async () => {
    const log = harness(() => data(0, new Uint8Array(), { eof: true }));
    log.follower.follow();
    await flush();
    log.follower.onLog({ dataBase64: b64([0xe2, 0x82]), offset: 0 });
    log.follower.onLog({ dataBase64: b64([0xac, 0x0a]), offset: 2 });
    assert.equal(log.sink.output, "€\n");
  });

  test("a fixed window scrolls to its top once written; following does not", async () => {
    const log = harness((span) => data(span.start, text("line\n"), { len: 3 * LOG_WINDOW, eof: true }));
    log.follower.goTo(0);
    await flush();
    assert.equal(log.sink.tops, 1);
    log.follower.follow();
    await flush();
    assert.equal(log.sink.tops, 1);
  });

  test("a window opening on a line boundary keeps its first line", async () => {
    const log = harness((span) => data(span.start, span.start === LOG_WINDOW ? text("first whole line\nsecond\n") : text(""), { len: 3 * LOG_WINDOW }));
    log.follower.goTo(LOG_WINDOW);
    await flush();
    assert.equal(log.sink.output, "first whole line\nsecond\n");
  });
});

test("follow and navigation continue short kept pages and preserve bytes on failure", async () => {
  for (const navigate of [false, true]) {
    const log = harness(({ start }) => {
      if (start === 0) return data(0, text("kept"), { len: 12 });
      if (start === 4) return data(4, text("tail"), { len: 12 });
      throw new LogPageUnavailable("host", 1, "unreachable");
    });
    if (navigate) log.follower.goTo(0);
    else log.follower.follow();
    await flush();
    assert.deepEqual(log.requests.map(({ start }) => start), [0, 4, 8]);
    assert.equal(log.sink.output, "kepttail");
    assert.equal(log.view().span.end, 8);
    assert.equal(log.view().loading, false);
    assert.equal(log.view().unavailable.host, "host");
  }
});

test("short reads stop when the server makes no progress", async () => {
  const log = harness(({ start }) => data(start, start === 0 ? text("kept") : text("")));
  log.follower.follow();
  await flush();
  assert.deepEqual(log.requests.map(({ start }) => start), [0, 4]);
  assert.equal(log.sink.output, "kept");
});

test("repeated generation resets stop and retain the last displayed bytes", async () => {
  const log = harness(({ start }, _, count) => count % 2 === 1
    ? data(start, text("kept"), { len: 8 })
    : { kind: "reset", info: { ident: "other-node", len: 8 } });
  log.follower.follow();
  for (let i = 0; i < 10; i++) await flush();
  assert.equal(log.requests.length, 6);
  assert.equal(log.sink.output, "kept");
  assert.match(log.view().error, /keeps changing/);
  log.follower.onReconnect();
  log.follower.onLog({ dataBase64: "", offset: 20 });
  await flush();
  assert.equal(log.requests.length, 6);
});

test("a failed navigation or missing host file retains the displayed window", async () => {
  let offline = false;
  const log = harness(({ start }) => offline
    ? { kind: "reset", info: null }
    : data(start, text("saved"), { len: 3 * LOG_WINDOW, eof: true }));
  log.follower.goTo(0);
  await flush();
  const shown = log.view().span;
  offline = true;
  log.follower.goTo(LOG_WINDOW);
  await flush();
  assert.equal(log.sink.output, "saved");
  assert.deepEqual(log.view().span, shown);
  assert.equal(log.view().loading, false);
});

test("an authoritative empty replacement clears the previous file", async () => {
  let empty = false;
  const log = harness((_, ident) => {
    if (empty && ident === "file-a") return { kind: "reset", info: { ident: "file-b", len: 0 } };
    return data(0, text(empty ? "" : "saved"), { ident: empty ? "file-b" : "file-a", len: empty ? 0 : 5, eof: true });
  });
  log.follower.follow();
  await flush();
  assert.equal(log.sink.output, "saved");
  empty = true;
  log.follower.onReconnect();
  await flush();
  assert.equal(log.sink.output, "");
  assert.deepEqual(log.view().span, { start: 0, end: 0 });
  assert.equal(log.view().len, 0);
});
