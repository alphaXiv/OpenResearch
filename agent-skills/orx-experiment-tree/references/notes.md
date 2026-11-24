# Experiment notes: `orx exp desc`

Each experiment node carries a free-form **description** (markdown), the same
field `create-experiment --description` sets. Use it for notes:
observations, hypotheses, or a running summary. It is a whole-document field:
writing overwrites whatever was there.

```sh
orx exp desc <expId>                          # print the description to stdout (empty → hint on stderr)
orx exp desc <expId> --set "tried lr=3e-4, diverged at step 4k"   # overwrite with a short note
cat notes.md | orx exp desc <expId> --stdin   # overwrite from stdin (long markdown)
```

- **Read** prints the text to **stdout** (pipe/redirect-friendly); when empty, a
  hint is printed to **stderr** and stdout stays empty.
- **Write** with exactly one of `--set` (inline) or `--stdin` (whole of stdin).
  Passing both is an error. Writing **replaces** the entire description; to
  append, read first, edit, and write back.
- `<expId>` comes from `orx create-experiment` output or
  `orx project view` (the experiment id, not a run or project id).
