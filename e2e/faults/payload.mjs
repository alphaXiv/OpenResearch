/**
 * An invented training run that reports the way an instrumented one does, in a variant that is healthy
 * or broken in one chosen way. The run command of an experiment: `node payload.mjs VARIANT [--tick MS]`.
 * One step takes `--tick` milliseconds (default 100), so a 600-step run lasts about a minute.
 *
 * Stages: setup (four shards), train (600 steps), eval (six tasks). Every line it prints is something
 * a real script prints; nothing in it says which variant it is, except the config line a person would see.
 */
const variant = process.argv[2] ?? 'healthy'
const tick = Number(process.argv[process.argv.indexOf('--tick') + 1]) || 100

const STEPS = 600
const SHARDS = 4
const TASKS = ['arc_easy', 'hellaswag', 'piqa', 'winogrande', 'boolq', 'copa']
const started = performance.now()

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const elapsed = () => ((performance.now() - started) / 1000).toFixed(2)
const out = (text) => process.stdout.write(`${text}\n`)
const err = (text) => process.stderr.write(`${text}\n`)
/** A number as Python's `json.dumps` writes it: NaN and Infinity stay bare. */
const num = (value) => (Number.isFinite(value) ? String(Number(value.toFixed(4))) : Number.isNaN(value) ? 'NaN' : value > 0 ? 'Infinity' : '-Infinity')
const field = (value) => (typeof value === 'number' ? num(value) : JSON.stringify(value))
const report = (record, measures) => {
  const top = Object.entries({ ...record, t: Number(elapsed()) }).map(([key, value]) => `${JSON.stringify(key)}:${field(value)}`)
  const own = measures ? `,"m":{${Object.entries(measures).map(([key, value]) => `${JSON.stringify(key)}:${field(value)}`).join(',')}}` : ''
  out(`::orx:: {${top.join(',')}${own}}`)
}

/** A repeatable stand-in for noise: the same step gives the same number in every run. */
const noise = (step) => {
  const x = Math.sin(step * 12.9898) * 43758.5453
  return x - Math.floor(x) - 0.5
}

const depth = variant === 'wrong-depth' ? 8 : 4
const lr = variant === 'lr-x100' ? 0.03 : 0.0003
const params = (depth * 7.35).toFixed(1)

/** The healthy loss curve: a fast fall that flattens near 0.9. */
const healthy = (step) => 0.9 + 3.1 * Math.exp(-step / 110) + 0.03 * noise(step)

function loss(step) {
  switch (variant) {
    case 'lr-x100':
      return step < 60 ? healthy(step) : Math.min(14, healthy(60) + 0.18 * (step - 60) ** 1.15 / 6) + 0.4 * noise(step)
    case 'nan-400':
      return step >= 400 ? NaN : healthy(step)
    case 'plateau':
      return step < 90 ? healthy(step) : healthy(90) - 0.0004 * (step - 90) + 0.03 * noise(step)
    default:
      return healthy(step)
  }
}

const pace = variant === 'slow-3x' ? tick * 3 : tick

out(`config: depth=${variant === 'wrong-depth' ? 4 : depth} width=512 heads=8 seq_len=1024 batch=32 lr=${lr.toExponential(2)} steps=${STEPS}`)
if (variant === 'wrong-depth') err('WARNING config: --depth 8 overrides DEPTH from the environment, which is set to 4; using 4')
out(`model: ${params} M parameters`)

report({ phase: 'setup', step: 0, of: SHARDS, unit: 'shards' })
for (let shard = 1; shard <= SHARDS; shard++) {
  await sleep(700)
  out(`downloading shard ${shard}/${SHARDS} ... ok (${(120 + 3 * shard).toFixed(0)} MB)`)
  report({ phase: 'setup', step: shard, of: SHARDS, unit: 'shards' })
}
out('dataset ready: 4 shards, 21.3 M tokens')

report({ phase: 'train', step: 0, of: STEPS, unit: 'steps' })
if (variant === 'hang') {
  out('compiling model with torch.compile (this can take a few minutes) ...')
  await sleep(2 ** 31 - 1)
}

for (let step = 1; step <= STEPS; step++) {
  if (variant === 'step-stuck' && step === 300) {
    // The step counter stops; the process keeps writing as if it were working.
    for (let attempt = 1; ; attempt++) {
      await sleep(1000)
      err(`WARNING dataloader: worker 3 timed out after 60.0s fetching batch ${step + 1}, retrying (attempt ${attempt})`)
      report({ step })
      out(`step ${step}/${STEPS} | waiting for batch | ${(attempt).toString()} retries`)
    }
  }
  if (variant === 'hang-mid' && step === 300) await sleep(2 ** 31 - 1)
  await sleep(pace)

  if (variant === 'crash' && step === 350) {
    err('Traceback (most recent call last):')
    err('  File "train.py", line 212, in <module>')
    err('    loss = model(x, y)')
    err('  File "model.py", line 88, in forward')
    err('    att = (q @ k.transpose(-2, -1)) * scale')
    err('torch.OutOfMemoryError: CUDA out of memory. Tried to allocate 2.00 GiB. GPU 0 has a total capacity of 23.65 GiB of which 1.12 GiB is free.')
    process.exit(1)
  }

  if (variant === 'warning-flood' && step >= 200) {
    if (step === 450) err('ERROR checkpoint: failed to write /ckpt/step450.pt (No space left on device); continuing without it')
    for (let n = 0; n < 5; n++) err(`WARNING data: skipped sample ${18000 + step * 7 + n} (corrupt jpeg header, 0x${(step * 7 + n).toString(16)})`)
  }

  const value = loss(step)
  const rate = 41000 * (variant === 'slow-3x' ? 1 / 3 : 1) + 800 * noise(step * 3)
  if (step % 5 === 0) {
    const measures = { loss: value, tok_per_sec: rate, lr }
    report({ phase: 'train', step, of: STEPS, unit: 'steps' }, step % 50 === 0 ? { ...measures, val_loss: value + 0.08 } : measures)
  }
  if (step % 20 === 0) out(`step ${step}/${STEPS} | loss ${Number.isFinite(value) ? value.toFixed(4) : String(value).toLowerCase()} | lr ${lr.toExponential(2)} | ${Math.round(rate)} tok/s`)
}

/** A model that trained badly scores near chance (0.25) on every task. */
const final = loss(STEPS)
const quality = Number.isFinite(final) ? Math.max(0, Math.min(1, 1 - (final - 0.9) / 4)) : 0
report({ phase: 'eval', step: 0, of: TASKS.length, unit: 'tasks' })
let total = 0
for (const [index, task] of TASKS.entries()) {
  await sleep(600)
  const good = 0.78 - index * 0.04 + 0.02 * noise(index + 40)
  const score = variant === 'eval-zeros' ? 0 : 0.25 + (good - 0.25) * quality
  total += score
  out(`eval ${task}: acc ${score.toFixed(4)}`)
  report({ phase: 'eval', step: index + 1, of: TASKS.length, unit: 'tasks', row: 'eval' }, { task, score })
}
report({ phase: 'eval', step: TASKS.length, of: TASKS.length, unit: 'tasks' }, { eval_score: total / TASKS.length })
out(`eval mean acc ${(total / TASKS.length).toFixed(4)}`)
out('done')
