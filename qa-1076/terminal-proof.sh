#!/bin/bash
set -euo pipefail
echo 'OpenResearch #1076 | Linux x86_64 | Real applications; no production uploads'
echo 'Setup WebSocket -> PTY -> app verification -> local HTTP telemetry receiver'
echo 'External OpenCode fixture: one anonymous free model; no authenticated providers'
echo
echo 'BEFORE   2567ddf9  main'
python3 -u qa-1076/prove.py "$RUNNER_TEMP/baseline/orx" before
echo
echo 'AFTER    12cc022e  PR #1076'
python3 -u qa-1076/prove.py "$GITHUB_WORKSPACE/target/debug/orx" after
echo
echo 'Before/after assertions passed, including the actual received interruption.'
