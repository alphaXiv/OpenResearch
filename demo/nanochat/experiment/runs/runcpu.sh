#!/bin/bash

# Showing an example run for exercising some of the code paths on the CPU (or MPS on Macbooks)
# This script was last updated/tuned on Jan 17, 2026.

# Run as:
# bash runs/runcpu.sh

# NOTE: Training LLMs requires GPU compute and $$$. You will not get far on your Macbook.
# Think of this run as educational/fun demo, not something you should expect to work well.
# You may also want to run this script manually and one by one, copy pasting commands into your terminal.

# Each stage reports itself to OpenResearch as an `::orx::` line; the Python scripts
# report their steps and measures inside it through orx_run.py.
report() { printf '::orx:: %s\n' "$1"; }
# Echoes a stage's output and reports a step for each line matching $1: its groups are
# the step and the total when it has them, otherwise matching lines are counted.
relay() {
    local n=0 line
    while IFS= read -r line; do
        printf '%s\n' "$line"
        [[ $line =~ $1 ]] || continue
        n=$((n + 1))
        report "{\"step\":${BASH_REMATCH[1]:-$n}${BASH_REMATCH[2]:+,\"of\":${BASH_REMATCH[2]}}}"
    done
}

# all the setup stuff
# Shards, tokenizers and uv's cache are identical for every run, so runs share them and
# only the first downloads and trains them. Checkpoints stay in the run's own directory.
CACHE="${ORX_CACHE_DIR:-$HOME/.cache/openresearch}"
DATA="$CACHE/nanochat/climbmix-8"
export NANOCHAT_BASE_DIR="$PWD/.cache/nanochat"
export UV_CACHE_DIR="$CACHE/uv"
export PYTHONUNBUFFERED=1
mkdir -p "$NANOCHAT_BASE_DIR" "$UV_CACHE_DIR" "$DATA"
report '{"phase":"install"}'
# uv ships no shell installer for Windows, and either installer drops the binary
# somewhere this shell was told about before the install ran.
if ! command -v uv &> /dev/null; then
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://astral.sh/uv/install.ps1 | iex" ;;
        *)
            curl -LsSf https://astral.sh/uv/install.sh | sh ;;
    esac
    export PATH="$HOME/.local/bin:$PATH"
fi
[ -d ".venv" ] || uv venv
uv sync --extra cpu
# A Windows venv keeps its activate script in Scripts/, not bin/.
source .venv/bin/activate 2> /dev/null || source .venv/Scripts/activate
if [ -z "$WANDB_RUN" ]; then
    WANDB_RUN=dummy
fi

report '{"phase":"download","step":0,"of":9,"unit":"shards"}'
NANOCHAT_BASE_DIR="$DATA" python -m nanochat.dataset -n 8 2>&1 | relay '^Successfully downloaded |^Skipping '
# Git Bash's ln -s copies, which is slower but just as correct.
[ -e "$NANOCHAT_BASE_DIR/base_data_climbmix" ] || ln -s "$DATA/base_data_climbmix" "$NANOCHAT_BASE_DIR/base_data_climbmix"

# train tokenizer on ~2B characters (~34 seconds on my MacBook Pro M3 Max)
# A tokenizer is reused only by runs with the same arguments, data and tokenizer code.
report '{"phase":"tokenizer","unit":"merges"}'
TOK_ARGS="--max-chars=2000000000"
TOKENIZER="$CACHE/nanochat/tokenizer-$({ echo "$TOK_ARGS $DATA"; cat uv.lock scripts/tok_train.py nanochat/tokenizer.py nanochat/dataset.py; } | cksum | cut -d ' ' -f 1)"
if [ -f "$TOKENIZER/token_bytes.pt" ]; then
    echo "Reusing the tokenizer in $TOKENIZER"
    ln -s "$TOKENIZER" "$NANOCHAT_BASE_DIR/tokenizer"
else
    python -m scripts.tok_train $TOK_ARGS 2>&1 | relay '\(([0-9]+)/([0-9]+) merges\)'
    if [ -f "$NANOCHAT_BASE_DIR/tokenizer/token_bytes.pt" ] && [ ! -e "$TOKENIZER" ]; then
        cp -R "$NANOCHAT_BASE_DIR/tokenizer" "$TOKENIZER.$$" && mv "$TOKENIZER.$$" "$TOKENIZER"
    fi
fi
python -m scripts.tok_eval

# train a small 6 layer model
# I tuned this run to complete in about 30 minutes on my MacBook Pro M3 Max.
# To get better results, try increasing num_iterations, or get other ideas from your favorite LLM.
report '{"phase":"train"}'
python -m scripts.base_train \
    --depth=6 \
    --head-dim=64 \
    --window-pattern=L \
    --max-seq-len=512 \
    --device-batch-size=32 \
    --total-batch-size=16384 \
    --eval-every=100 \
    --eval-tokens=524288 \
    --core-metric-every=-1 \
    --sample-every=100 \
    --num-iterations=5000 \
    --run=$WANDB_RUN
report '{"phase":"eval","step":0,"of":22,"unit":"tasks"}'
python -m scripts.base_eval --device-batch-size=1 --split-tokens=16384 --max-per-task=16 2>&1 | relay '^Evaluating: .* accuracy: '

# Train SFT with a fresh optimizer and memory-safe periodic evaluation.
report '{"phase":"sft"}'
python -m scripts.chat_sft \
    --load-optimizer=0 \
    --eval-every=200 \
    --eval-tokens=524288 \
    --chatcore-every=-1 \
    --num-iterations=1500 \
    --run=$WANDB_RUN

# Chat with the model over CLI
# The model should be able to say that it is Paris.
# It might even know that the color of the sky is blue.
# Sometimes the model likes it if you first say Hi before you ask it questions.
report '{"phase":"chat"}'
python -m scripts.chat_cli -p "What is the capital of France?"
