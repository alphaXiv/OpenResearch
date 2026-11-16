#!/bin/bash

set -euo pipefail

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
relay() {
    local n=0 line
    while IFS= read -r line; do
        printf '%s\n' "$line"
        [[ $line =~ $1 ]] || continue
        n=$((n + 1))
        report "{\"step\":$n}"
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
BASE_STEPS=5000
SFT_STEPS=1500
SHARDS=8
report "{\"plan\":[\
{\"phase\":\"install\",\"of\":1,\"unit\":\"environments\"},\
{\"phase\":\"download\",\"of\":$((SHARDS + 1)),\"unit\":\"shards\"},\
{\"phase\":\"tokenizer\",\"of\":2,\"unit\":\"commands\"},\
{\"phase\":\"train\",\"of\":$BASE_STEPS,\"unit\":\"steps\"},\
{\"phase\":\"eval\",\"of\":1,\"unit\":\"commands\"},\
{\"phase\":\"sft\",\"of\":$((SFT_STEPS - 1)),\"unit\":\"steps\"},\
{\"phase\":\"chat\",\"of\":1,\"unit\":\"prompts\"}]}"
report '{"phase":"install","step":0,"of":1,"unit":"environments"}'
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
report '{"step":1}'
if [ -z "${WANDB_RUN:-}" ]; then
    WANDB_RUN=dummy
fi

report "{\"phase\":\"download\",\"step\":0,\"of\":$((SHARDS + 1)),\"unit\":\"shards\"}"
NANOCHAT_BASE_DIR="$DATA" python -m nanochat.dataset -n "$SHARDS" 2>&1 | relay '^Successfully downloaded |^Skipping '
# Git Bash's ln -s copies, which is slower but just as correct.
[ -e "$NANOCHAT_BASE_DIR/base_data_climbmix" ] || ln -s "$DATA/base_data_climbmix" "$NANOCHAT_BASE_DIR/base_data_climbmix"

# train tokenizer on ~2B characters (~34 seconds on my MacBook Pro M3 Max)
# A tokenizer is reused only by runs with the same arguments, data and tokenizer code.
report '{"phase":"tokenizer","step":0,"of":2,"unit":"commands"}'
TOK_ARGS="--max-chars=2000000000"
TOKENIZER="$CACHE/nanochat/tokenizer-$({ echo "$TOK_ARGS $DATA"; cat uv.lock scripts/tok_train.py nanochat/tokenizer.py nanochat/dataset.py; } | cksum | cut -d ' ' -f 1)"
if [ -f "$TOKENIZER/token_bytes.pt" ]; then
    echo "Reusing the tokenizer in $TOKENIZER"
    [ -e "$NANOCHAT_BASE_DIR/tokenizer" ] || ln -s "$TOKENIZER" "$NANOCHAT_BASE_DIR/tokenizer"
else
    python -m scripts.tok_train $TOK_ARGS
    if [ -f "$NANOCHAT_BASE_DIR/tokenizer/token_bytes.pt" ] && [ ! -e "$TOKENIZER" ]; then
        cp -R "$NANOCHAT_BASE_DIR/tokenizer" "$TOKENIZER.$$" && mv "$TOKENIZER.$$" "$TOKENIZER"
    fi
fi
report '{"step":1}'
python -m scripts.tok_eval
report '{"step":2}'

# train a small 6 layer model
# The recorded run took 131.55 minutes for base training and 39.07 for SFT.
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
    --num-iterations="$BASE_STEPS" \
    --run=$WANDB_RUN
report '{"phase":"eval","step":0,"of":1,"unit":"commands"}'
python -m scripts.base_eval --device-batch-size=1 --split-tokens=16384 --max-per-task=16
report '{"step":1}'

# Train SFT with a fresh optimizer and memory-safe periodic evaluation.
report '{"phase":"sft"}'
python -m scripts.chat_sft \
    --load-optimizer=0 \
    --eval-every=200 \
    --eval-tokens=524288 \
    --chatcore-every=-1 \
    --num-iterations="$SFT_STEPS" \
    --run=$WANDB_RUN

# Chat with the model over CLI
# The model should be able to say that it is Paris.
# It might even know that the color of the sky is blue.
# Sometimes the model likes it if you first say Hi before you ask it questions.
report '{"phase":"chat","step":0,"of":1,"unit":"prompts"}'
python -m scripts.chat_cli -p "What is the capital of France?"
report '{"step":1}'
