# nanochat demo evidence

This compact evidence pack comes from the completed Apple-Silicon run recorded in the demo. It preserves the small outputs needed to inspect the experiment without bundling the multi-gigabyte training workspace.

## Included

- `training-metrics.csv` — base and SFT loss, validation BPB, throughput, and elapsed time by step
- `evaluation-metrics.json` — base BPB, completed CORE task results, and final headline metrics
- `checkpoints/base/meta_005000.json` — metadata saved with the final base checkpoint
- `checkpoints/sft/meta_001499.json` — metadata saved with the final SFT checkpoint
- `tokenizer/tokenizer.pkl` and `tokenizer/token_bytes.pt` — the tokenizer produced by this run
- `final-inference.txt` — the recorded prompt and response from the final SFT checkpoint
- `run-manifest.json` — an inventory of included and omitted run outputs

`training-metrics.csv` and `evaluation-metrics.json` are regenerated from the recorded run log with `node scripts/generate-demo-evidence.mjs`. The checkpoint metadata, tokenizer files, inference transcript, and manifest are preserved directly from the recorded run. Because `tokenizer.pkl` uses Python's pickle format, load it only through nanochat's trusted tokenizer code.

## Intentionally omitted

The model weights, optimizer states, downloaded datasets, evaluation corpus, Python environment, and package cache are not bundled. They account for several gigabytes and normally remain in the run-local workspace rather than the Git repository or project artifacts.

The manifest records the exact checkpoint paths, byte sizes, and SHA-256 hashes so follow-up analysis can distinguish files that were produced from files that are locally available. Run `bash runs/runcpu.sh` in a fresh experiment to reproduce the complete workspace.
