"""Advisory run signals. Vendor with `orx skill compute/assets/orx_run.py`."""

import atexit
import contextlib
import json
import math
import numbers
import os
import sys
import threading
import time


_lock = threading.RLock()
# One clock per process, from its first progress(), so `t` restarts at zero only in a new process.
_origin = None
_warned = set()
_NAMED = "a name is letters, digits and _, at most 64, not starting with a digit"


def _warn(problems):
    """Says once on stderr each thing the run prints that orx will ignore. It never raises: it runs
    inside the user's training."""
    try:
        for problem in problems:
            if problem not in _warned:
                _warned.add(problem)
                sys.stderr.write(f"orx_run: orx ignores {problem}\n")
                sys.stderr.flush()
    except Exception:
        pass


def _shown(value):
    text = repr(value)
    return text if len(text) <= 40 else text[:39] + "…"


def _identifier(name):
    return (isinstance(name, str) and 0 < len(name) <= 64 and name.isascii()
            and (name[0].isalpha() or name[0] == "_")
            and all(c.isalnum() or c == "_" for c in name))


def _stage_problems(name, total, unit):
    """What a stage report or plan entry holds that the reader drops, each as (what, rule)."""
    if name is not None and not (isinstance(name, str) and 0 < len(name.encode()) <= 128):
        yield f"the stage name {_shown(name)}", "a stage name is 1 to 128 bytes"
    if total is not None and not (isinstance(total, numbers.Real) and not isinstance(total, bool)
                                  and math.isfinite(total) and total > 0):
        yield f"the total {_shown(total)}", "a total is a number above 0"
    if unit is not None and not (isinstance(unit, str) and len(unit.encode()) <= 64):
        yield f"the unit {_shown(unit)}", "a unit is text of at most 64 bytes"


def _cells(name, cells):
    if name is not None and not _identifier(name):
        yield f"the table {_shown(name)}: {_NAMED}"
    for key, value in cells.items():
        if not _identifier(key):
            yield f"{_shown(key)}: {_NAMED}"
        elif isinstance(value, bool):
            yield f"{key}={value}: report 1 or 0"
        elif isinstance(value, str) and len(value.encode()) > 1024:
            yield f"{key}: its text is over 1024 bytes"
        elif not isinstance(value, (int, float, str, type(None))):
            yield f"{key}, a {type(value).__name__}: report a number or text"


def _stage(total, unit):
    """A stage's total and unit, leaving out each one not given so the phase keeps what it has."""
    return {key: value for key, value in (("of", total), ("unit", unit)) if value is not None}


def plan(stages, rank=None):
    """Declares every stage the run goes through, in order, replacing any plan declared before.

    Each stage is a name, or a dict with "phase" and optionally "of" and "unit":
    plan(["install", {"phase": "train", "of": 5000, "unit": "steps"}, "eval"]).
    A stage that runs again is listed again where it recurs, never twice in a row.
    """
    listed = []
    for stage in stages:
        stage = {"phase": stage} if isinstance(stage, str) else dict(stage)
        name = stage.get("phase")
        if not isinstance(name, str) or not name:
            raise ValueError(f"each stage needs a phase name, got {stage!r}")
        if listed and listed[-1]["phase"] == name:
            raise ValueError(f"{name!r} is listed twice in a row; list it again only after another stage")
        listed.append(stage)

    def problems():
        if len(listed) > 64:
            yield f"the whole plan: it lists {len(listed)} stages, at most 64"
        for stage in listed:
            for what, rule in _stage_problems(stage["phase"], stage.get("of"), stage.get("unit")):
                yield f"the whole plan for {what}: {rule}"

    if rank is None:
        rank = int(os.environ.get("RANK") or os.environ.get("SLURM_PROCID") or "0")
    if rank == 0:
        _warn(problems())
        with _lock:
            sys.stdout.write("::orx:: " + json.dumps({"plan": listed}, separators=(",", ":")) + "\n")
            sys.stdout.flush()


class progress:
    def __init__(self, total=None, unit=None, rank=None, interval=1.0, start=0, phase=None):
        if rank is None:
            rank = int(os.environ.get("RANK") or os.environ.get("SLURM_PROCID") or "0")
        self._enabled = rank == 0
        self._interval = max(0.0, float(interval))
        self._pending = None
        self._lock = _lock
        with self._lock:
            global _origin
            if _origin is None:
                _origin = time.monotonic()
            # Naming the phase here lets a script with no shell around it start in a named stage.
            named = {} if phase is None else {"phase": phase}
            if self._enabled:
                _warn(f"{what}: {rule}" for what, rule in _stage_problems(phase, total, unit))
            self._write({**named, "step": start, **_stage(total, unit)})
        atexit.register(self.flush)

    @staticmethod
    def _metrics(metrics):
        result = {}
        for key, value in metrics.items():
            if not isinstance(value, (str, int, float, bool, type(None))) and hasattr(value, "item"):
                value = value.item()
            json.dumps(value, allow_nan=True)
            result[key] = value
        return result

    def _write(self, record):
        if self._enabled:
            record = {**record, "t": time.monotonic() - _origin}
            sys.stdout.write("::orx:: " + json.dumps(record, allow_nan=True, separators=(",", ":")) + "\n")
            sys.stdout.flush()
        self._last = time.monotonic()

    def flush(self):
        with self._lock:
            if self._pending is not None:
                self._write(self._pending)
                self._pending = None

    def step(self, step, **metrics):
        with self._lock:
            metrics = self._metrics(metrics)
            if self._enabled:
                _warn(_cells(None, metrics))
            merged = {} if self._pending is None else self._pending["m"].copy()
            merged.update({key: value for key, value in metrics.items() if value is not None})
            self._pending = {"step": step, "m": merged}
            nonfinite = any(isinstance(v, float) and not math.isfinite(v) or
                            isinstance(v, str) and v in ("NaN", "Infinity", "-Infinity")
                            for v in metrics.values())
            if nonfinite or time.monotonic() - self._last >= self._interval:
                self.flush()

    @contextlib.contextmanager
    def phase(self, name, total=None, unit=None):
        with self._lock:
            self.flush()
            if self._enabled:
                _warn(f"{what}: {rule}" for what, rule in _stage_problems(name, total, unit))
            self._write({"phase": name, "step": 0, **_stage(total, unit)})
        try:
            yield self
        finally:
            self.flush()

    def row(self, name, **cells):
        with self._lock:
            self.flush()
            cells = self._metrics(cells)
            if self._enabled:
                _warn(_cells(name, cells))
            self._write({"row": name, "m": cells})

    def spend(self, cumulative_usd):
        with self._lock:
            self.flush()
            self._write({"spend": cumulative_usd})
