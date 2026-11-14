/**
 * Deliberately broken runs. Each scenario launches a healthy parent, then a child under it that
 * fails in one way (`payload.mjs` prints both), and is sampled at fixed seconds after the child starts.
 * @module
 */

export type Scenario = {
  id: string
  /** The child's experiment title. The varying setting comes first, as agents are told to write it. */
  title: string
  /** What goes wrong, and where in the run: the one sentence the timeline is read against. */
  fault: string
  /** Seconds after the child starts at which the timeline is read. */
  points: number[]
  /** The child is stopped here if it has not ended, the way its agent would. */
  stopAt: number
  /** The chat's agent, woken by the run's end, marks it as being repaired before it answers. */
  revise?: boolean
}

const STEADY = [4, 10, 20, 30, 40, 48, 56, 64, 72, 80]

export const scenarios: Scenario[] = [
  {
    id: 'control',
    title: 'Control',
    fault: 'Nothing: a healthy run that must draw no flag. Its line and strip are what every other scenario is read against.',
    points: STEADY,
    stopAt: 100,
  },
  {
    id: 'lr-x100',
    title: 'Learning rate 3e-2',
    fault: 'Learning rate 100 times the parent. A normal start; the loss turns up at step 60 (about 9 s) and climbs to 14, finite, to the end.',
    points: STEADY,
    stopAt: 100,
  },
  {
    id: 'nan-400',
    title: 'Warmup 0',
    fault: 'The loss is NaN from step 400 (about 43 s) on; the process keeps stepping and reporting NaN to the end.',
    points: [4, 20, 30, 40, 44, 48, 56, 64, 72],
    stopAt: 100,
    revise: true,
  },
  {
    id: 'hang',
    title: 'Compile mode max-autotune',
    fault: 'After setup the training script prints "compiling model" at step 0 and then nothing, forever, with the process alive.',
    points: [6, 20, 45, 75, 88, 96, 120, 150, 180],
    stopAt: 185,
  },
  {
    id: 'hang-mid',
    title: 'Pin memory off',
    fault: 'At step 300 (about 33 s) the process stops writing anything, with the process alive: the same silence as hang, with progress behind it.',
    points: [20, 36, 60, 90, 120, 130, 150],
    stopAt: 155,
  },
  {
    id: 'step-stuck',
    title: 'Dataloader workers 8',
    fault: 'At step 300 (about 33 s) the step counter stops. Each second the log gets a dataloader retry warning and a step-300 heartbeat, so the log never goes quiet.',
    points: [4, 20, 36, 60, 100, 126, 140, 170],
    stopAt: 175,
  },
  {
    id: 'slow-3x',
    title: 'Gradient checkpointing on',
    fault: 'Every training step takes three times as long as the parent’s, from the first. Numbers are healthy; it would finish in about 3 minutes against 1.',
    points: [6, 15, 30, 45, 60, 90, 120, 150],
    stopAt: 155,
  },
  {
    id: 'plateau',
    title: 'Weight decay 0.3',
    fault: 'The loss follows the parent to step 90 (about 12 s), then barely moves: 2.2 at step 300 where the parent is at 1.1.',
    points: STEADY,
    stopAt: 100,
  },
  {
    id: 'wrong-depth',
    title: 'Depth 8',
    fault: 'The title says depth 8 and the command passes --depth 8, but the script uses 4 (a warning at the top, then "depth=4 ... 29.4 M parameters"). Its loss, throughput and eval are the parent’s to the digit: only the brief says it is wrong.',
    points: STEADY,
    stopAt: 100,
  },
  {
    id: 'eval-zeros',
    title: 'Eval on the held-out split',
    fault: 'Training is healthy. The eval stage prints acc 0.0000 for every task and its score measure is 0, with no error.',
    points: [4, 20, 40, 60, 66, 68, 70, 74, 80],
    stopAt: 100,
  },
  {
    id: 'crash',
    title: 'Batch size 64',
    fault: 'Step 350 (about 38 s): a Python traceback, torch.OutOfMemoryError, exit status 1.',
    points: [4, 20, 34, 38, 40, 44, 50],
    stopAt: 100,
  },
  {
    id: 'warning-flood',
    title: 'Augmentation jpeg',
    fault: 'From step 200 (about 23 s) every step writes five WARNING lines of one new family ("skipped sample N (corrupt jpeg header)"); about 2,000 in all. At step 450 one ERROR line of another family (a checkpoint that could not be written) is buried in it. Numbers are healthy.',
    points: [4, 20, 26, 32, 40, 48, 56, 64, 72],
    stopAt: 100,
  },
]
