# Local dev instance, isolated from your normal `orx` data/ports (see AGENTS.md).
# Requires `just` (https://github.com/casey/just).

# Start (or reuse) a dev slot with a copy of your local CLI database and open the dashboard.
up:
    node scripts/dev-slot.mjs start --db copy --open

# Stop this worktree's dev slot.
down:
    node scripts/dev-slot.mjs stop

# Show this worktree's dev slot status.
status:
    node scripts/dev-slot.mjs status
