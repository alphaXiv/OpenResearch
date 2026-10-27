import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import test from 'node:test';

const script = readFileSync(new URL('../../src/commands/desktop_zoom.js', import.meta.url), 'utf8');

function harness({ mac = false, frame = false } = {}) {
  const messages = [];
  const forwarded = [];
  const listeners = new Map();
  let ready;
  const window = {
    addEventListener(type, handler, capture) {
      if (type === 'keydown') assert.equal(capture, true);
      listeners.set(type, handler);
    },
    ipc: { postMessage: (message) => messages.push(message) },
  };
  window.top = frame ? { postMessage: (...args) => forwarded.push(args) } : window;
  runInNewContext(script.replace('__ORX_ZOOM_PLATFORM__', mac ? 'macos' : 'windows'), {
    window,
    document: { addEventListener(type, handler, options) {
      assert.equal(type, 'DOMContentLoaded');
      assert.equal(options.once, true);
      ready = handler;
    } },
  });
  return { messages, forwarded, listeners, ready };
}

function press(key, options = {}, environment = {}) {
  const context = harness(environment);
  let prevented = false;
  let stopped = false;
  context.listeners.get('keydown')({ key, ctrlKey: true, ...options,
    preventDefault() { prevented = true; },
    stopImmediatePropagation() { stopped = true; },
  });
  return { messages: context.messages, forwarded: context.forwarded, prevented, stopped };
}

test('Ctrl zoom shortcuts consume the key and forward exactly one native command', () => {
  for (const [key, action] of [['=', 'in'], ['+', 'in'], ['-', 'out'], ['0', 'reset']]) {
    assert.deepEqual(press(key), { messages: [`page-zoom:${action}`], forwarded: [], prevented: true, stopped: true });
  }
  assert.deepEqual(press('+', { shiftKey: true }).messages, ['page-zoom:in']);
});

test('unrelated input and macOS menu accelerators are left alone', () => {
  const ignored = { messages: [], forwarded: [], prevented: false, stopped: false };
  for (const options of [{ ctrlKey: false }, { altKey: true }, { metaKey: true }, { isComposing: true }]) {
    assert.deepEqual(press('=', options), ignored);
  }
  assert.deepEqual(press('0', { shiftKey: true }), ignored);
  assert.deepEqual(press('x'), ignored);
  assert.deepEqual(press('=', {}, { mac: true }), ignored);
  assert.deepEqual(press('=', {}, { mac: true, frame: true }), ignored);
});

test('sandboxed preview shortcuts reach the main frame without using frame IPC', () => {
  const main = harness();
  for (const [key, action] of [['=', 'in'], ['-', 'out'], ['0', 'reset']]) {
    const result = press(key, {}, { frame: true });
    assert.deepEqual(result.messages, []);
    assert.deepEqual(result.forwarded, [[`page-zoom:${action}`, '*']]);
    main.listeners.get('message')({ data: result.forwarded[0][0] });
  }
  assert.deepEqual(main.messages, ['page-zoom:in', 'page-zoom:out', 'page-zoom:reset']);
  for (const data of ['page-zoom:sync', 'titlebar:close', null, { action: 'page-zoom:in' }]) {
    main.listeners.get('message')({ data });
  }
  assert.equal(main.messages.length, 3);
  assert.equal(harness({ frame: true }).ready, undefined);
});

test('a reloaded document requests the current host zoom instead of replaying a startup value', () => {
  const { ready, messages } = harness();
  assert.deepEqual(messages, []);
  ready();
  assert.deepEqual(messages, ['page-zoom:sync']);
});
