# /// script
# requires-python = ">=3.10"
# dependencies = ["torch==2.9.1", "rustbpe==0.1.0", "tiktoken==0.11.0", "numpy==2.2.6"]
# [tool.uv.sources]
# torch = { index = "pytorch-cpu" }
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# ///
"""A real, short nanochat learning-rate comparison on bundled text."""
import copy
import csv
import json
import logging
import math
from pathlib import Path
import time
import sys

started = time.perf_counter()
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import torch
from nanochat.gpt import GPT, GPTConfig
from nanochat.tokenizer import RustBPETokenizer


def plot(rows, output):
    from orx_chart import render_chart
    render_chart(output, title="Tiny nanochat · learning-rate comparison",
                 subtitle="0.8M parameters · AdamW · 20 steps per run · single seed",
                 metrics=[dict(key="val_loss", label="Validation loss", unit="nats/token"),
                          dict(key="train_loss", label="Training loss", unit="nats/token")],
                 series=[dict(name=f"LR {lr}", points=[row for row in rows if row["lr"] == lr])
                         for lr in (0.001, 0.002)])


def main():
    logging.getLogger("rustbpe").setLevel(logging.WARNING)
    torch.set_num_threads(2)
    torch.manual_seed(42)
    text = (Path(__file__).parent.parent / "README.md").read_text(encoding="utf-8")
    split = int(len(text) * 0.8)
    tokenizer = RustBPETokenizer.train_from_iterator([text[:split]], 1024)
    train = torch.tensor(tokenizer.encode(text[:split]), dtype=torch.long)
    valid = torch.tensor(tokenizer.encode(text[split:]), dtype=torch.long)
    config = GPTConfig(sequence_len=64, vocab_size=1024, n_layer=2, n_head=2, n_kv_head=2, n_embd=128, window_pattern="L")
    model = GPT(config)
    model.init_weights()
    models = {0.001: model, 0.002: copy.deepcopy(model)}
    optimizers = {lr: torch.optim.AdamW(item.parameters(), lr=lr, weight_decay=0.01) for lr, item in models.items()}
    params = sum(p.numel() for p in model.parameters())
    print(f"Tiny nanochat: {params:,} parameters; 2 layers; width 128; vocab 1,024; context 64", flush=True)
    print("20 steps per arm on bundled nanochat README text; CPU; identical initialization and batches.", flush=True)
    print("Early optimization demo only: not a coherent chatbot or a comparison with the recorded 73.5M model.", flush=True)

    def batch(data, generator):
        starts = torch.randint(len(data) - 64, (4,), generator=generator)
        sequences = torch.stack([data[start:start + 65] for start in starts])
        return sequences[:, :-1].contiguous(), sequences[:, 1:].contiguous()

    generator = torch.Generator().manual_seed(123)
    validation = batch(valid, torch.Generator().manual_seed(456))
    rows = []
    training_started = time.perf_counter()
    for step in range(21):
        x, y = batch(train, generator)
        for lr, item in models.items():
            val_loss = None
            if step % 5 == 0:
                item.eval()
                with torch.no_grad():
                    val_loss = item(*validation).item()
                assert math.isfinite(val_loss)
            train_loss = None
            if step < 20:
                item.train()
                optimizers[lr].zero_grad(set_to_none=True)
                loss = item(x, y)
                train_loss = loss.item()
                assert math.isfinite(train_loss)
                loss.backward()
                optimizers[lr].step()
            rows.append(dict(step=step, lr=lr, train_loss=train_loss, val_loss=val_loss))
            print(f"step {step:02d}/20 | lr {lr} | train_loss {train_loss} | val_loss {val_loss}", flush=True)
    assert rows[0]["val_loss"] == rows[1]["val_loss"], "Comparison must start from identical weights"
    output = Path("artifacts")
    output.mkdir(exist_ok=True)
    with (output / "tiny-nanochat-loss.csv").open("w", newline="", encoding="utf-8") as file:
        writer = csv.DictWriter(file, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    plot(rows, output / "tiny-nanochat-loss.html")
    summary = dict(parameters=params, steps_per_arm=20, optimizer="AdamW", seed=42,
                   training_seconds=time.perf_counter() - training_started,
                   process_seconds=time.perf_counter() - started,
                   results=[row for row in rows if row["step"] == 20])
    (output / "tiny-nanochat-results.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2), flush=True)
    print(f"Results and ready-made loss figure: {output.resolve()}", flush=True)


if __name__ == "__main__":
    main()
