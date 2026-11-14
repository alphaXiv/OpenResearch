"""Advisory run signals. Vendor with `orx skill compute/assets/orx_run.py`."""

import atexit
import contextlib
import json
import math
import os
import sys
import threading
import time


_lock = threading.RLock()
# One clock per process, from its first progress(), so `t` restarts at zero only in a new process.
_origin = None


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
    if rank is None:
        rank = int(os.environ.get("RANK") or os.environ.get("SLURM_PROCID") or "0")
    if rank == 0:
        with _lock:
            sys.stdout.write("::orx:: " + json.dumps({"plan": listed}, separators=(",", ":")) + "\n")
            sys.stdout.flush()


class progress:
    def __init__(self, total=None, unit=None, rank=None, interval=1.0, start=0):
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
            self._write({"step": start, **_stage(total, unit)})
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
            self._write({"phase": name, "step": 0, **_stage(total, unit)})
        try:
            yield self
        finally:
            self.flush()

    def row(self, name, **cells):
        with self._lock:
            self.flush()
            self._write({"row": name, "m": self._metrics(cells)})

    def spend(self, cumulative_usd):
        with self._lock:
            self.flush()
            self._write({"spend": cumulative_usd})
