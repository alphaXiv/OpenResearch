import assert from 'node:assert/strict';
import test from 'node:test';
import { comparisonRows, metricDelta, changedParameters, summarize } from '../src/features/research/comparison.ts';

const run = (id, extra = {}) => ({ id, experimentId: 'e', projectId: 'p', status: 'done', commitSha: 'abc', createdAt: 0, updatedAt: 0, cancelRequested: false, backend: { kind: 'colab_job', flavor: 't4' }, ...extra });
const metric = (seed, value = 0, extra = {}) => ({ version: 1, dataset: 'v1', split: 'test', parameters: { lr: .01 }, seed, metrics: { f1: value }, ...extra });

test('groups independent completed seeds and uses sample deviation', () => {
  const rows = comparisonRows([run('a'), run('b')], { a: metric(1, 2), b: metric(2, 4) }, true);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].metrics.f1.mean, 3);
  assert.ok(Math.abs(rows[0].metrics.f1.sd - Math.sqrt(2)) < 1e-12);
  assert.equal(rows[0].metrics.f1.n, 2);
});
test('does not combine datasets, splits, configurations, commits or compute flavors', () => {
  for (const change of [{ dataset: 'v2' }, { split: 'validation' }, { parameters: { lr: .1 } }, { metrics: { loss: 2 } }]) {
    assert.equal(comparisonRows([run('a'), run('b')], { a: metric(1), b: metric(2, 4, change) }, true).length, 2);
  }
  for (const change of [{ commitSha: 'def' }, { experimentId: 'other' }, { status: 'failed' }, { backend: { kind: 'colab_job', flavor: 'a100' } }]) {
    assert.equal(comparisonRows([run('a'), run('b', change)], { a: metric(1), b: metric(2) }, true).length, 2);
  }
});
test('repeated or missing seeds remain individual runs', () => {
  for (const seeds of [[1, 1], [null, null]]) {
    assert.equal(comparisonRows([run('a'), run('b')], { a: metric(seeds[0]), b: metric(seeds[1]) }, true).length, 2);
  }
});
test('zero measurements survive and missing measurements stay absent', () => {
  const rows = comparisonRows([run('a'), run('b')], { a: metric(1, 0) }, false);
  assert.equal(rows[0].metrics.f1.mean, 0);
  assert.equal(rows[0].metrics.f1.sd, null);
  assert.deepEqual(Object.keys(rows[1].metrics), []);
  assert.equal(metricDelta(rows[1], rows[0], 'f1'), null);
});
test('differences require comparable evaluation contexts', () => {
  const rows = comparisonRows([run('a'), run('b'), run('c')], { a: metric(1, .8), b: metric(2, .9), c: metric(3, .9, { split: 'train' }) }, false);
  assert.ok(Math.abs(metricDelta(rows[1], rows[0], 'f1') - .1) < 1e-12);
  assert.equal(metricDelta(rows[2], rows[0], 'f1'), null);
});
test('parameter differences include additions, removals and changed values', () => {
  const rows = comparisonRows([run('a'), run('b')], { a: metric(1, 0, { parameters: { lr: .01, removed: true } }), b: metric(2, 0, { parameters: { lr: .1, added: null } }) }, false);
  assert.deepEqual(changedParameters(rows[1], rows[0]), ['added', 'lr', 'removed']);
});
test('grouping is independent of parameter key order and runtime identity', () => {
  const rows = comparisonRows([run('a'), run('b', { backend: { kind: 'colab_job', flavor: 't4', jobId: 'different' } })], { a: metric(1, 0, { parameters: { lr: .01, batch: 32 } }), b: metric(2, 0, { parameters: { batch: 32, lr: .01 } }) }, true);
  assert.equal(rows.length, 1);
});
test('empty and nonfinite samples cannot produce fabricated scores', () => {
  assert.equal(summarize([]), null);
  assert.equal(summarize([NaN, Infinity]), null);
  assert.deepEqual(summarize([0]), { mean: 0, sd: null, n: 1 });
});

test('metric names matching object properties remain ordinary measurements', () => {
  const metrics = JSON.parse('{"__proto__":0.7,"constructor":0.8}');
  const [row] = comparisonRows([run('a')], { a: metric(1, 0, { metrics }) }, false);
  assert.deepEqual(Object.keys(row.metrics).sort(), ['__proto__', 'constructor']);
  assert.equal(row.metrics.__proto__.mean, 0.7);
  assert.equal(row.metrics.constructor.mean, 0.8);
});
