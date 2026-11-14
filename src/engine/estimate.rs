use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct Range {
    pub lo_ms: f64,
    pub mid_ms: f64,
    pub hi_ms: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Phase {
    pub name: String,
    pub total: Option<f64>,
    pub prior: Option<Range>,
    /// Expected total API spend for this phase; absence means unknown.
    pub spend: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct Point {
    pub at: f64,
    pub done: f64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) enum Machine {
    Unpriced,
    Unknown,
    Priced {
        usd_per_hour: f64,
        quantum_ms: f64,
        billed_from: Option<f64>,
        uncertain: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) enum Spend {
    NeverReported,
    Reported { usd: f64, at_phase_start: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InputError {
    NonFinite {
        field: &'static str,
        index: Option<usize>,
        value: f64,
    },
    BelowMinimum {
        field: &'static str,
        index: Option<usize>,
        value: f64,
        minimum: f64,
    },
    AboveMaximum {
        field: &'static str,
        index: Option<usize>,
        value: f64,
        maximum: f64,
    },
    NonPositive {
        field: &'static str,
        index: Option<usize>,
        value: f64,
    },
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (field, index, value) = match *self {
            Self::NonFinite {
                field,
                index,
                value,
            }
            | Self::BelowMinimum {
                field,
                index,
                value,
                ..
            }
            | Self::AboveMaximum {
                field,
                index,
                value,
                ..
            }
            | Self::NonPositive {
                field,
                index,
                value,
            } => (field, index, value),
        };
        write!(f, "{field}")?;
        if let Some(index) = index {
            write!(f, "[{index}]")?;
        }
        write!(f, " = {value}: ")?;
        match self {
            Self::NonFinite { .. } => write!(f, "must be finite"),
            Self::BelowMinimum { minimum, .. } => write!(f, "must be at least {minimum}"),
            Self::AboveMaximum { maximum, .. } => write!(f, "must be at most {maximum}"),
            Self::NonPositive { .. } => write!(f, "must be positive"),
        }
    }
}

impl std::error::Error for InputError {}

fn bounded(
    field: &'static str,
    index: Option<usize>,
    value: f64,
    minimum: f64,
    maximum: f64,
) -> Result<(), InputError> {
    if !value.is_finite() {
        return Err(InputError::NonFinite {
            field,
            index,
            value,
        });
    }
    if value < minimum {
        return Err(InputError::BelowMinimum {
            field,
            index,
            value,
            minimum,
        });
    }
    if value > maximum {
        return Err(InputError::AboveMaximum {
            field,
            index,
            value,
            maximum,
        });
    }
    Ok(())
}

fn checked<T>(result: Result<T, InputError>, errors: &mut Vec<InputError>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error);
            None
        }
    }
}

fn clock_time(field: &'static str, value: f64, at: f64) -> Result<f64, InputError> {
    if value.is_finite() && value > at && value - at <= 5000.0 {
        return Ok(at);
    }
    bounded(field, None, value, 0.0, at).map(|()| value)
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Input {
    pub now: f64,
    pub started_at: f64,
    /// Unknown when the phase began in bytes read after the fact.
    pub phase_started_at: Option<f64>,
    /// Last authoritative observation; freezes elapsed and cost during contact loss.
    pub as_of: f64,
    pub contact_lost: bool,
    pub phases: Vec<Phase>,
    pub current: usize,
    /// Observed reports only, including resumed counts; never insert a phase origin.
    pub points: Vec<Point>,
    /// Whether `points` are placed in time. Unplaced points keep their durations, so a range
    /// is known, but where they lie, and so any stall, is not.
    pub placed: bool,
    pub ends_at: Option<f64>,
    pub machine: Machine,
    pub gpu_count: Option<f64>,
    pub spend: Spend,
    pub spend_per_hour: Option<f64>,
    /// `phases` is the run's declared plan, stages to come included: a stage's weight is its
    /// prior, so whole-run progress is unknown unless every stage has one, and a stage with no
    /// prior leaves time left a floor.
    #[serde(default)]
    pub planned: bool,
}

#[derive(Debug)]
pub(crate) struct Eta {
    pub range: Option<Range>,
    /// Last valid progress time, or `cost.at` when no timing is available.
    pub at: f64,
    pub pending: bool,
    /// Overrun from `cost.at`: `now` while live, `as_of` after contact loss.
    pub past_limit_ms: Option<f64>,
    /// The range leaves out a stage to come whose length is unknown: the time left is at least it.
    pub floor: bool,
}

#[derive(Debug)]
pub(crate) struct Segment {
    pub name: String,
    pub weight: f64,
    pub fill: Option<f64>,
}

#[derive(Debug)]
pub(crate) struct Cost {
    pub so_far: Option<f64>,
    pub projected: Option<f64>,
    pub projected_range: Option<[f64; 2]>,
    pub at_limit: Option<f64>,
    pub spend: Option<f64>,
    /// Allocated GPU count × payload-active hours, independent of billing setup.
    pub gpu_hours: Option<f64>,
    pub uncertain: bool,
    pub at: f64,
}

#[derive(Debug)]
pub(crate) struct Output {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Equal to `whole` until phases have priors")
    )]
    pub phase_fraction: Option<f64>,
    pub segments: Vec<Segment>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Weights are all estimated until priors")
    )]
    pub weights_estimated: bool,
    pub whole: Option<f64>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Equal to `estimate` until priors")
    )]
    pub current_phase: Option<Eta>,
    pub estimate: Option<Eta>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Declared stop rules gate on it")
    )]
    pub completed_slices: usize,
    pub stall_ms: Option<f64>,
    pub errors: Vec<InputError>,
    pub cost: Cost,
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n == 0 {
        0.0
    } else {
        (sorted[(n - 1) / 2] + sorted[n / 2]) / 2.0
    }
}

fn deviation(values: &[f64], center: f64, floor: f64) -> f64 {
    let residuals: Vec<_> = values.iter().map(|v| (v - center).abs()).collect();
    (median(&residuals) * 1.4826).max(floor)
}

fn phase_range(phase: &Phase, points: &[Point]) -> (Option<Range>, usize) {
    let Some(total) = phase.total else {
        return (None, 0);
    };
    let done = points.last().map_or(0.0, |p| p.done);
    let remaining = (1.0 - done / total).max(0.0);
    let prior = phase.prior.map(|r| Range {
        lo_ms: (r.mid_ms * 0.75).min(r.lo_ms) * remaining,
        mid_ms: r.mid_ms * remaining,
        hi_ms: (r.mid_ms * 1.3).max(r.hi_ms) * remaining,
    });
    let bins = orx_signals::slice_count(total);
    let Some(first) = points.first() else {
        return (prior, 0);
    };
    let width = total / bins as f64;
    let mut crossings = Vec::new();
    let mut j = 0;
    let mut last_bin = 0;
    for k in 0..=bins {
        let goal = orx_signals::slice_boundary(total, k);
        if goal > done {
            break;
        }
        // A relative tolerance cannot turn a tiny partial slice into a full one.
        let tolerance = 4.0 * f64::EPSILON * goal.abs().max(first.done.abs());
        let at_first = (goal - first.done).abs() <= tolerance;
        if goal < first.done && !at_first {
            continue;
        }
        last_bin = k;
        if at_first {
            crossings.push(first.at);
            continue;
        }
        while j + 2 < points.len() && points[j + 1].done < goal {
            j += 1;
        }
        if j + 1 >= points.len() {
            break;
        }
        let a = points[j];
        let b = points[j + 1];
        let fraction = (goal - a.done) / (b.done - a.done);
        crossings.push(a.at + fraction * (b.at - a.at));
    }
    let durations: Vec<_> = crossings.windows(2).map(|p| p[1] - p[0]).collect();
    let n = durations.len();
    if done >= total {
        return (
            Some(Range {
                lo_ms: 0.0,
                mid_ms: 0.0,
                hi_ms: 0.0,
            }),
            n,
        );
    }
    // The partial first slice is absent; the first observed full slice is warm-up.
    let usable = &durations[1.min(n)..];
    if usable.is_empty() {
        if prior.is_some() {
            return (prior, n);
        }
        let last = points.last().unwrap();
        if done <= first.done || last.at <= first.at {
            return (None, n);
        }
        let mid_ms = (total - done) * (last.at - first.at) / (done - first.done);
        return (
            Some(Range {
                lo_ms: mid_ms * 0.5,
                mid_ms,
                hi_ms: mid_ms * 1.6,
            }),
            n,
        );
    }
    let window = &usable[usable.len().saturating_sub(14)..];
    let center = median(window);
    let mad = deviation(window, center, center * 0.01);
    let level = window
        .iter()
        .map(|v| v.clamp(center - 3.0 * mad, center + 3.0 * mad))
        .sum::<f64>()
        / window.len() as f64;
    if level <= 0.0 {
        return (prior, n);
    }
    let mut slope = 0.0;
    let mut last_level = level;
    let mut noise = mad.max(level * 0.01);
    if window.len() >= 5 {
        let mut slopes = Vec::new();
        for (i, a) in window.iter().enumerate() {
            for (j, b) in window.iter().enumerate().skip(i + 1) {
                slopes.push((b - a) / (j - i) as f64);
            }
        }
        let trend = median(&slopes);
        let residuals: Vec<_> = window
            .iter()
            .enumerate()
            .map(|(i, y)| y - trend * i as f64)
            .collect();
        let intercept = median(&residuals);
        let spread = deviation(&residuals, intercept, level * 0.01);
        let tau = slopes
            .iter()
            .map(|v| if *v == 0.0 { 0.0 } else { v.signum() })
            .sum::<f64>()
            / slopes.len() as f64;
        if tau.abs() >= 0.6 && trend.abs() * (window.len() - 1) as f64 > 3.0 * spread {
            slope = trend;
            last_level = intercept + trend * (window.len() - 1) as f64;
            noise = spread;
        }
    }
    let sum = |trend: f64| {
        let predict =
            |k: usize| (0.3 * level).max(last_level + trend * (k as f64 - (last_bin as f64 - 1.0)));
        let partial =
            (1.0 - (done - orx_signals::slice_boundary(total, last_bin)) / width).clamp(0.0, 1.0);
        partial * predict(last_bin) + ((last_bin + 1)..bins).map(predict).sum::<f64>()
    };
    let mut mid_ms = sum(slope);
    let half = sum(slope / 2.0);
    let relative =
        (0.03 + 1.3 * (noise / level) / (window.len() as f64).sqrt() + 0.35 / (n + 1) as f64)
            .clamp(0.04, 0.60);
    let mut lo_ms = mid_ms.min(half) * (1.0 - relative);
    let mut hi_ms = mid_ms.max(half) * (1.0 + relative);
    if let Some(prior) = prior.filter(|_| n < 4) {
        let blend = (4 - n) as f64 / 4.0;
        mid_ms = mid_ms * (1.0 - blend) + prior.mid_ms * blend;
        lo_ms = lo_ms.min(mid_ms * 0.75);
        if let Some(r) = phase.prior.filter(|r| r.lo_ms < r.mid_ms * 0.75) {
            lo_ms = lo_ms.min(r.lo_ms * remaining);
        }
        hi_ms = hi_ms.max(mid_ms * 1.3);
        if let Some(r) = phase.prior.filter(|r| r.hi_ms > r.mid_ms * 1.3) {
            hi_ms = hi_ms.max(r.hi_ms * remaining);
        }
    }
    (
        Some(Range {
            lo_ms,
            mid_ms,
            hi_ms,
        }),
        n,
    )
}

/// All timestamps and durations are milliseconds; observations belong to one phase occurrence.
pub(crate) fn estimate(input: &Input) -> Result<Output, InputError> {
    if input.current >= input.phases.len() {
        return Err(InputError::AboveMaximum {
            field: "current",
            index: None,
            value: input.current as f64,
            maximum: input.phases.len() as f64 - 1.0,
        });
    }
    bounded("now", None, input.now, 0.0, f64::INFINITY)?;
    bounded("as_of", None, input.as_of, 0.0, input.now)?;
    let at = if input.contact_lost {
        input.as_of
    } else {
        input.now
    };
    let mut errors = Vec::new();
    let started_at = checked(clock_time("started_at", input.started_at, at), &mut errors);
    let phase_started_at = input
        .phase_started_at
        .and_then(|start| checked(clock_time("phase_started_at", start, at), &mut errors));
    let elapsed = started_at.map(|start| at - start);
    let ends_at = input.ends_at.and_then(|end| {
        checked(
            bounded("ends_at", None, end, 0.0, f64::INFINITY),
            &mut errors,
        )
        .map(|()| end)
    });
    let mut phases = input.phases.clone();
    for (index, phase) in phases.iter_mut().enumerate() {
        if let Some(value) = phase.total {
            let result = if value == 0.0 {
                Err(InputError::NonPositive {
                    field: "total",
                    index: Some(index),
                    value,
                })
            } else {
                bounded("total", Some(index), value, 0.0, f64::INFINITY)
            };
            phase.total = checked(result, &mut errors).map(|()| value);
        }
        if let Some(spend) = phase.spend {
            phase.spend = checked(
                bounded("phase.spend", Some(index), spend, 0.0, f64::INFINITY),
                &mut errors,
            )
            .map(|()| spend);
        }
        if let Some(r) = phase.prior {
            let mid = checked(
                bounded("prior.mid_ms", Some(index), r.mid_ms, 0.0, f64::INFINITY),
                &mut errors,
            );
            let lo = checked(
                bounded("prior.lo_ms", Some(index), r.lo_ms, 0.0, r.mid_ms),
                &mut errors,
            );
            let hi = checked(
                bounded("prior.hi_ms", Some(index), r.hi_ms, r.mid_ms, f64::INFINITY),
                &mut errors,
            );
            if mid.is_none() || lo.is_none() || hi.is_none() {
                phase.prior = None;
            }
        }
    }
    let mut points = Vec::<Point>::new();
    let mut previous_done = 0.0;
    let mut previous_at = 0.0;
    let mut points_valid = true;
    let mut progress_valid = true;
    let latest = if input.placed {
        input.as_of
    } else {
        f64::INFINITY
    };
    for (index, point) in input.points.iter().enumerate() {
        if checked(
            bounded(
                "points.done",
                Some(index),
                point.done,
                previous_done,
                f64::INFINITY,
            ),
            &mut errors,
        )
        .is_none()
        {
            progress_valid = false;
            points_valid = false;
        }
        if checked(
            bounded("points.at", Some(index), point.at, previous_at, latest),
            &mut errors,
        )
        .is_none()
        {
            points_valid = false;
        }
        previous_done = point.done;
        previous_at = point.at;
        if points.last().is_some_and(|last| last.at == point.at) {
            points.pop();
        }
        points.push(*point);
    }
    let (spend, spend_projection_valid) = match input.spend {
        Spend::NeverReported => (None, true),
        Spend::Reported {
            usd,
            at_phase_start,
        } => {
            let amount = checked(
                bounded("spend.usd", None, usd, 0.0, f64::INFINITY),
                &mut errors,
            )
            .map(|()| usd);
            let baseline = checked(
                bounded(
                    "spend.at_phase_start",
                    None,
                    at_phase_start,
                    0.0,
                    f64::INFINITY,
                ),
                &mut errors,
            );
            let monotonic = checked(
                bounded("spend.usd", None, usd, at_phase_start, f64::INFINITY),
                &mut errors,
            );
            (amount, baseline.is_some() && monotonic.is_some())
        }
    };
    let gpu_count = input.gpu_count.and_then(|count| {
        checked(
            bounded("gpu_count", None, count, 0.0, f64::INFINITY),
            &mut errors,
        )
        .map(|()| count)
    });
    let spend_per_hour = input.spend_per_hour.and_then(|rate| {
        checked(
            bounded("spend_per_hour", None, rate, 0.0, f64::INFINITY),
            &mut errors,
        )
        .map(|()| rate)
    });
    let machine = match input.machine {
        Machine::Priced {
            usd_per_hour,
            quantum_ms,
            billed_from,
            uncertain,
        } => {
            let rate = checked(
                bounded("usd_per_hour", None, usd_per_hour, 0.0, f64::INFINITY),
                &mut errors,
            );
            let quantum = checked(
                if quantum_ms == 0.0 {
                    Err(InputError::NonPositive {
                        field: "quantum_ms",
                        index: None,
                        value: quantum_ms,
                    })
                } else {
                    bounded("quantum_ms", None, quantum_ms, 0.0, f64::INFINITY)
                },
                &mut errors,
            );
            let origin = billed_from
                .and_then(|from| checked(clock_time("billed_from", from, at), &mut errors));
            if rate.is_some() && quantum.is_some() {
                Machine::Priced {
                    usd_per_hour,
                    quantum_ms,
                    billed_from: origin,
                    uncertain,
                }
            } else {
                Machine::Unknown
            }
        }
        machine => machine,
    };
    let phase = &phases[input.current];
    let open = input.phases[input.current].total.is_none();
    let observed = input.points.last().map(|p| p.done);
    let done = observed.unwrap_or(0.0);
    let total = phase.total;
    let fraction = total
        .filter(|_| progress_valid)
        .zip(observed)
        .map(|(total, done)| (done / total).min(1.0));
    let limit_fill = elapsed
        .zip(ends_at)
        .zip(started_at)
        .and_then(|((elapsed, end), start)| {
            if end <= start {
                None
            } else {
                Some((elapsed / (end - start)).clamp(0.0, 1.0))
            }
        });
    // A stage in progress with no total is timed against its prior, when it has one.
    let prior_fill = phase
        .prior
        .zip(phase_started_at)
        .filter(|_| input.planned && open)
        .map(|(prior, start)| {
            if prior.mid_ms > 0.0 {
                ((at - start) / prior.mid_ms).clamp(0.0, 1.0)
            } else {
                1.0
            }
        });
    let open_fill = if input.planned && phases.len() > 1 {
        prior_fill
    } else {
        prior_fill.or(limit_fill)
    };
    let weights_estimated = phases.iter().any(|p| p.prior.is_none());
    let weight_sum = if weights_estimated {
        phases.len() as f64
    } else {
        phases.iter().map(|p| p.prior.unwrap().mid_ms).sum()
    };
    let weights_estimated = weights_estimated || weight_sum == 0.0;
    let segments: Vec<_> = phases
        .iter()
        .enumerate()
        .map(|(i, p)| Segment {
            name: p.name.clone(),
            weight: if weights_estimated {
                1.0 / phases.len() as f64
            } else {
                p.prior.unwrap().mid_ms / weight_sum
            },
            fill: if i < input.current {
                Some(1.0)
            } else if i == input.current {
                if open {
                    open_fill
                } else {
                    fraction
                }
            } else {
                Some(0.0)
            },
        })
        .collect();
    let whole = if input.planned && weights_estimated && phases.len() > 1 {
        None
    } else if open && !(input.planned && phases.len() > 1) {
        open_fill
    } else {
        segments
            .iter()
            .try_fold(0.0, |sum, s| Some(sum + s.weight * s.fill?))
    };
    let mut gaps = Vec::new();
    let mut last_progress = points.first().map(|p| p.at);
    let mut last_report = None;
    for (index, pair) in input.points.windows(2).enumerate() {
        let progressed = pair[1].done > pair[0].done;
        if progressed {
            if let Some(previous) = last_report {
                let gap = pair[1].at - previous;
                if gap > 0.0 {
                    gaps.push(gap);
                }
            }
            last_progress = Some(pair[1].at);
        }
        if index == 0 || progressed {
            last_report = Some(pair[1].at);
        }
    }
    let cadence = if gaps.is_empty() {
        phase
            .prior
            .map(|prior| prior.mid_ms / total.map_or(1, orx_signals::slice_count) as f64)
    } else {
        Some(median(&gaps[gaps.len().saturating_sub(12)..]))
    };
    let silence = if input.placed {
        last_progress.or(phase_started_at)
    } else {
        phase_started_at.filter(|_| points.is_empty())
    }
    .map(|last| (at - last).max(0.0));
    let complete = progress_valid && total.is_some_and(|total| done >= total);
    let stall_ms = if complete {
        Some(0.0)
    } else if points_valid {
        cadence.zip(silence).map(|(gap, silence)| {
            if silence > 90_000.0_f64.max(4.0 * gap) {
                silence
            } else {
                0.0
            }
        })
    } else {
        None
    };
    // Repeated unchanged reports must not refit the last range during a stall.
    let timed = &points[..points
        .iter()
        .rposition(|p| Some(p.at) == last_progress)
        .map_or(0, |i| i + 1)];
    let (current_range, completed_slices) = if points_valid {
        phase_range(phase, timed)
    } else {
        (None, 0)
    };
    // A planned stage with no total is expected to last as long as its prior.
    let current_range = current_range.or_else(|| {
        let prior = phase.prior.filter(|_| input.planned && open)?;
        let spent = phase_started_at.map_or(0.0, |start| (at - start).max(0.0));
        Some(Range {
            lo_ms: (prior.lo_ms.min(prior.mid_ms * 0.85) - spent).max(0.0),
            mid_ms: (prior.mid_ms - spent).max(0.0),
            hi_ms: (prior.hi_ms.max(prior.mid_ms * 1.2) - spent).max(0.0),
        })
    });
    let estimate_at = if timed.is_empty() || !points_valid || !input.placed {
        at
    } else {
        last_progress.unwrap_or(at)
    };
    let finish = |remaining: f64| at + remaining;
    let eta = |range: Option<Range>, floor: bool| Eta {
        range,
        at: estimate_at,
        pending: range.is_none(),
        past_limit_ms: range
            .filter(|_| !floor)
            .and_then(|r| ends_at.map(|end| (finish(r.hi_ms) - end).max(0.0))),
        floor,
    };
    let later = |range: Range, prior: Range| Range {
        lo_ms: range.lo_ms + prior.lo_ms.min(prior.mid_ms * 0.85),
        mid_ms: range.mid_ms + prior.mid_ms,
        hi_ms: range.hi_ms + prior.hi_ms.max(prior.mid_ms * 1.2),
    };
    let mut total_range = current_range;
    // A plan's stage with no prior is left out, so what is left is at least the rest.
    let mut floor = false;
    if input.planned {
        // A stage in progress with neither a total nor a prior is unknown too.
        if current_range.is_none() && open {
            floor = true;
            total_range = Some(Range {
                lo_ms: 0.0,
                mid_ms: 0.0,
                hi_ms: 0.0,
            });
        }
        for stage in phases.iter().skip(input.current + 1) {
            match stage.prior {
                Some(prior) => total_range = total_range.map(|r| later(r, prior)),
                None => floor = true,
            }
        }
        // Nothing known at all is no estimate, not "at least 0s".
        if floor && total_range.is_some_and(|r| r.hi_ms == 0.0) {
            total_range = None;
        }
    } else {
        for stage in phases.iter().skip(input.current + 1) {
            total_range = total_range
                .zip(stage.prior)
                .map(|(r, prior)| later(r, prior));
        }
    }
    let machine_cost = |end: f64| match machine {
        Machine::Unpriced => Some(0.0),
        Machine::Unknown => None,
        Machine::Priced {
            usd_per_hour,
            quantum_ms,
            billed_from,
            ..
        } => billed_from.map(|start| {
            let billed = ((end - start).max(0.0) / quantum_ms).ceil() * quantum_ms;
            usd_per_hour * billed / 3_600_000.0
        }),
    };
    let prior_start = input.current + usize::from(matches!(input.spend, Spend::Reported { .. }));
    let mut future = phases.iter().skip(prior_start);
    let future_spend = match input.spend {
        Spend::NeverReported => Some(future.filter_map(|phase| phase.spend).sum::<f64>()),
        Spend::Reported { .. } => future.try_fold(0.0, |sum, phase| Some(sum + phase.spend?)),
    };
    let projected_spend = match input.spend {
        Spend::NeverReported => future_spend,
        Spend::Reported {
            usd,
            at_phase_start,
        } => total
            .filter(|_| progress_valid && spend_projection_valid && done > 0.0)
            .zip(future_spend)
            .map(|(total, future)| {
                usd + (usd - at_phase_start) / done * (total - done).max(0.0) + future
            }),
    };
    let has_usd = !matches!(machine, Machine::Unpriced)
        || spend.is_some()
        || phases
            .iter()
            .skip(prior_start)
            .any(|phase| phase.spend.is_some());
    let projection = |remaining: f64| {
        if !has_usd {
            return None;
        }
        machine_cost(finish(remaining))
            .zip(projected_spend)
            .map(|(machine, spend)| machine + spend)
    };
    let api_only = if matches!(machine, Machine::Unpriced) && total.is_some() && has_usd {
        projected_spend
    } else {
        None
    };
    // A floor on the time left is no projection of what the run will cost.
    let total_range_known = total_range.filter(|_| !floor);
    let projected = total_range_known
        .and_then(|r| projection(r.mid_ms))
        .or(api_only);
    let projected_range = total_range_known
        .and_then(|r| Some([projection(r.lo_ms)?, projection(r.hi_ms)?]))
        .or(api_only.map(|spend| [spend, spend]));
    let at_limit = ends_at.filter(|_| has_usd).and_then(|end| {
        let spend = if open && spend.is_some() {
            spend_per_hour.zip(future_spend).map(|(rate, future)| {
                spend.unwrap() + rate * (end - at).max(0.0) / 3_600_000.0 + future
            })
        } else {
            projected_spend
        };
        machine_cost(end)
            .zip(spend)
            .map(|(machine, spend)| machine + spend)
    });
    let uncertain = !errors.is_empty()
        || matches!(input.spend, Spend::NeverReported)
        || input.contact_lost
        || matches!(
            machine,
            Machine::Unknown
                | Machine::Priced {
                    billed_from: None,
                    ..
                }
                | Machine::Priced {
                    uncertain: true,
                    ..
                }
        )
        || (spend.is_some() && projected_spend.is_none());
    Ok(Output {
        phase_fraction: fraction,
        segments,
        weights_estimated,
        whole,
        current_phase: input.phases[input.current]
            .total
            .map(|_| eta(current_range, false)),
        estimate: if input.planned {
            match total_range {
                Some(_) => Some(eta(total_range, floor)),
                // Pending only while the stage's own pace is still being learned.
                None => (input.phases[input.current].total.is_some() && current_range.is_none())
                    .then(|| eta(None, false)),
            }
        } else {
            input.phases[input.current]
                .total
                .map(|_| eta(total_range, false))
        },
        completed_slices,
        stall_ms,
        errors,
        cost: Cost {
            so_far: if !matches!(machine, Machine::Unpriced) || spend.is_some() {
                machine_cost(at).map(|machine| machine + spend.unwrap_or(0.0))
            } else {
                None
            },
            projected,
            projected_range,
            at_limit,
            spend,
            gpu_hours: gpu_count
                .zip(elapsed)
                .map(|(gpus, elapsed)| gpus * elapsed / 3_600_000.0),
            uncertain,
            at,
        },
    })
}

/// One stage of a run with a declared plan, as shown: one the run went through, one to come, or
/// one it skipped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stage {
    pub name: String,
    pub of: Option<f64>,
    pub unit: Option<String>,
    /// The index in the fold's phases of the occurrence that is this stage, once the run reached it.
    pub phase: Option<usize>,
    pub planned: bool,
    pub skipped: bool,
    /// How long the parent's run took over the same stage.
    pub prior_ms: Option<f64>,
}

/// Lays a run's phase occurrences over its plan, in order. An occurrence is the next planned stage
/// of its name, and the planned stages it passed over were skipped; one the plan does not name is
/// unplanned, and the plan grows by it there. A run that finished skipped the stages it never
/// reached. A stage's prior is how long the parent took over the stage of the same name and count.
pub(crate) fn stages(
    plan: &[orx_signals::Stage],
    signals: Option<&orx_signals::Snapshot>,
    finished: bool,
    parent: Option<(&orx_signals::Snapshot, bool)>,
) -> Vec<Stage> {
    let planned = |stage: &orx_signals::Stage, skipped: bool| Stage {
        name: stage.phase.clone(),
        of: stage.of,
        unit: stage.unit.clone(),
        phase: None,
        planned: true,
        skipped,
        prior_ms: None,
    };
    let mut shown = Vec::new();
    let mut next = 0;
    for (index, occurrence) in signals.map_or(&[][..], |s| &s.phases).iter().enumerate() {
        let found = plan[next..]
            .iter()
            .position(|stage| stage.phase == occurrence.name);
        let stage = found.map(|offset| {
            shown.extend(plan[next..next + offset].iter().map(|s| planned(s, true)));
            next += offset + 1;
            &plan[next - 1]
        });
        shown.push(Stage {
            name: occurrence.name.clone(),
            of: occurrence.of.or(stage.and_then(|s| s.of)),
            unit: occurrence
                .unit
                .clone()
                .or_else(|| stage.and_then(|s| s.unit.clone())),
            phase: Some(index),
            planned: stage.is_some(),
            skipped: false,
            prior_ms: None,
        });
    }
    shown.extend(plan[next..].iter().map(|s| planned(s, finished)));

    let Some((parent, done)) = parent else {
        return shown;
    };
    let last = parent.phases.len().checked_sub(1);
    let took: Vec<(&str, Option<f64>)> = parent
        .phases
        .iter()
        .enumerate()
        .map(|(index, phase)| {
            // The last stage of a run that finished ended with its log.
            let end = phase.ended_at.or(parent
                .timing
                .last_log_at
                .filter(|_| done && Some(index) == last));
            let took = phase
                .started_at
                .zip(end)
                .map(|(start, end)| (end - start) as f64);
            (phase.name.as_str(), took.filter(|ms| *ms >= 0.0))
        })
        .collect();
    let mut counted: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for stage in &mut shown {
        let count = counted.entry(stage.name.clone()).or_default();
        stage.prior_ms = took
            .iter()
            .filter(|(name, _)| *name == stage.name)
            .nth(*count)
            .and_then(|(_, took)| *took);
        *count += 1;
    }
    shown
}

/// What the laptop knows of a run besides its signals, on the laptop's clock.
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub started_at: f64,
    pub contact_lost: bool,
    pub ends_at: Option<f64>,
    pub machine: Machine,
    pub gpu_count: Option<f64>,
    pub spend: Spend,
    pub spend_per_hour: Option<f64>,
}

/// Evaluates a run in its host's time. Every time in a checkpoint is on the clock that folded
/// it; `clock` is the `(host, laptop)` pair from the last successful collection, which maps the
/// laptop's `now` and the run's own times onto that clock, so skew of any size changes nothing
/// and output times are the host's. The pair's host time is what the data is known as of.
/// Without a pair the checkpoint is read on its own clock, at its last time once it has ended.
/// A report is placed only through the fold's anchor.
pub(crate) fn with_signals(
    run: Run,
    signals: &orx_signals::Snapshot,
    plan: &[Stage],
    clock: Option<(f64, f64)>,
    now: f64,
) -> Input {
    let (as_of, offset) = match clock {
        Some((host, laptop)) => (host, host - laptop),
        None => (signals.updated_at as f64, 0.0),
    };
    let ended = signals.extra.get("terminal") == Some(&serde_json::Value::Bool(true));
    let now = if clock.is_none() && ended {
        as_of
    } else {
        now + offset
    };
    let host = |laptop: f64| laptop + offset;
    let machine = match run.machine {
        Machine::Priced {
            usd_per_hour,
            quantum_ms,
            billed_from,
            uncertain,
        } => Machine::Priced {
            usd_per_hour,
            quantum_ms,
            billed_from: billed_from.map(host),
            uncertain,
        },
        machine => machine,
    };
    let mut input = Input {
        now,
        started_at: host(run.started_at),
        phase_started_at: signals.timing.started_ms.map(|ms| ms as f64),
        as_of,
        contact_lost: run.contact_lost,
        phases: vec![Phase {
            name: String::new(),
            total: None,
            prior: None,
            spend: None,
        }],
        current: 0,
        points: Vec::new(),
        placed: true,
        ends_at: run.ends_at.map(host),
        machine,
        gpu_count: run.gpu_count,
        spend: run.spend,
        spend_per_hour: run.spend_per_hour,
        planned: false,
    };
    // Stages to come and skipped ones have no reports; skipped ones take no time.
    let shown: Vec<&Stage> = plan.iter().filter(|stage| !stage.skipped).collect();
    let planned = |input: &mut Input| {
        input.phases = shown
            .iter()
            .map(|stage| Phase {
                name: stage.name.clone(),
                total: stage.of,
                prior: stage.prior_ms.map(|ms| Range {
                    lo_ms: ms,
                    mid_ms: ms,
                    hi_ms: ms,
                }),
                spend: None,
            })
            .collect();
        input.current = shown
            .iter()
            .position(|stage| {
                stage.phase.is_some() && stage.phase == signals.phases.len().checked_sub(1)
            })
            .unwrap_or(0);
        input.planned = true;
    };
    let Some(phase) = signals.phases.last() else {
        if !shown.is_empty() {
            planned(&mut input);
        }
        return input;
    };

    // `t` gives only differences; the anchor's read time is what places them.
    let offset = if phase.timing_source == "observed" {
        Some(0.0)
    } else {
        signals
            .timing
            .anchor
            .filter(|anchor| anchor.0 == phase.occurrence)
            .map(|(_, t, at)| at as f64 - t * 1000.0)
    };
    let to_point = |&(_, t, done): &(usize, f64, f64)| Point {
        at: offset.map_or(t * 1000.0, |offset| (t * 1000.0 + offset).min(as_of)),
        done,
    };
    input.points = signals
        .timing
        .progress_points
        .iter()
        .filter(|point| point.0 == phase.occurrence)
        .map(to_point)
        .collect();
    input.placed = offset.is_some();

    let first_report = input.points.first().filter(|_| input.placed);
    input.phase_started_at = phase
        .started_at
        .map(|ms| ms as f64)
        .or(first_report.map(|point| point.at));
    // Several bars leave the whole run open: one stage that fills against its time limit, since
    // how many bars are still to come is unknown. So does a full bar or pattern-read counter on a
    // run still going.
    let full = matches!(phase.source.as_str(), "detected" | "pattern")
        && !ended
        && phase
            .of
            .zip(phase.done)
            .is_some_and(|(of, done)| done >= of);
    if signals.several_bars() || full {
        input.phases[0].name = phase.name.clone();
        return input;
    }
    if !shown.is_empty() {
        planned(&mut input);
        return input;
    }
    input.phases = signals
        .phases
        .iter()
        .map(|occurrence| Phase {
            name: occurrence.name.clone(),
            total: occurrence.of,
            prior: None,
            spend: None,
        })
        .collect();
    input.current = signals.phases.len() - 1;
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        let tolerance = 1e-5_f64.max(expected.abs() * 1e-9);
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} != {expected}"
        );
    }

    fn optional_close(actual: Option<f64>, expected: &serde_json::Value) {
        match (actual, expected.as_f64()) {
            (Some(actual), Some(expected)) => close(actual, expected),
            (None, None) => (),
            other => panic!("{other:?}"),
        }
    }

    fn input(total: Option<f64>) -> Input {
        Input {
            now: 100_000.0,
            as_of: 100_000.0,
            started_at: 0.0,
            phase_started_at: Some(0.0),
            contact_lost: false,
            phases: vec![Phase {
                name: "train".into(),
                total,
                prior: None,
                spend: None,
            }],
            current: 0,
            points: (1..=10)
                .map(|step| Point {
                    at: step as f64 * 10_000.0,
                    done: step as f64,
                })
                .collect(),
            placed: true,
            ends_at: None,
            machine: Machine::Unpriced,
            gpu_count: Some(2.0),
            spend: Spend::NeverReported,
            spend_per_hour: None,
            planned: false,
        }
    }

    fn slice_reports(durations: &[f64]) -> Input {
        let mut i = input(Some(40.0));
        i.points = vec![Point {
            at: 10_000.0,
            done: 1.0,
        }];
        let mut at = 10_000.0;
        for (index, duration) in durations.iter().enumerate() {
            at += duration;
            i.points.push(Point {
                at,
                done: index as f64 + 2.0,
            });
        }
        i.now = at;
        i.as_of = at;
        i
    }

    fn priced() -> Machine {
        Machine::Priced {
            usd_per_hour: 3600.0,
            quantum_ms: 1.0,
            billed_from: Some(0.0),
            uncertain: false,
        }
    }

    #[test]
    fn prototype_reference_moments() {
        #[derive(Deserialize)]
        struct Moment {
            name: String,
            point_count: usize,
            now: f64,
            spend: Option<f64>,
            expected: Option<serde_json::Value>,
            original: Option<serde_json::Value>,
            whole: Option<f64>,
            reason: Option<String>,
            details: serde_json::Value,
        }
        #[derive(Deserialize)]
        struct Fixture {
            input: serde_json::Value,
            points: Vec<[f64; 2]>,
            segments: Vec<(String, f64)>,
            moments: Vec<Moment>,
        }
        let fixtures: Vec<Fixture> =
            serde_json::from_str(include_str!("../../tests/estimate/prototype.json")).unwrap();
        for fixture in fixtures {
            for moment in fixture.moments {
                if moment.reason.is_none() {
                    assert_eq!(moment.expected, moment.original, "{}", moment.name);
                }
                let mut value = fixture.input.clone();
                value["points"] = serde_json::Value::Array(
                    fixture.points[..moment.point_count]
                        .iter()
                        .map(|p| serde_json::json!({"at":p[0],"done":p[1]}))
                        .collect(),
                );
                value["now"] = serde_json::json!(moment.now);
                value["as_of"] = value["now"].clone();
                value["placed"] = serde_json::json!(true);
                value["spend"] = moment.spend.map_or(
                    serde_json::json!("NeverReported"),
                    |usd| serde_json::json!({"Reported":{"usd":usd,"at_phase_start":0.0}}),
                );
                let i: Input = serde_json::from_value(value).unwrap();
                let out = estimate(&i).unwrap();
                optional_close(out.whole, &serde_json::json!(moment.whole));
                assert!(out.errors.is_empty(), "{} {:?}", moment.name, out.errors);
                let d = &moment.details;
                let anchor = i.points.last().map_or(i.now, |p| p.at);
                match (out.current_phase.as_ref(), d["current"].as_array()) {
                    (None, None) => assert!(d["current"].is_null()),
                    (Some(eta), Some(bounds)) => {
                        close(eta.at, anchor);
                        assert!(!eta.pending);
                        let r = eta.range.unwrap();
                        for (actual, want) in [r.lo_ms, r.mid_ms, r.hi_ms].into_iter().zip(bounds) {
                            close(actual, want.as_f64().unwrap());
                        }
                    }
                    (Some(eta), None) => {
                        assert!(eta.pending);
                        close(eta.at, anchor);
                    }
                    other => panic!("{} {other:?}", moment.name),
                }
                if let Some(eta) = &out.current_phase {
                    optional_close(eta.past_limit_ms, &d["past"][0]);
                }
                if let Some(eta) = &out.estimate {
                    close(eta.at, anchor);
                    optional_close(eta.past_limit_ms, &d["past"][1]);
                }
                assert_eq!(out.segments.len(), fixture.segments.len());
                for (index, (segment, (name, weight))) in
                    out.segments.iter().zip(&fixture.segments).enumerate()
                {
                    assert_eq!(&segment.name, name);
                    close(segment.weight, *weight);
                    optional_close(segment.fill, &d["fills"][index]);
                }
                optional_close(out.stall_ms, &d["stall"]);
                assert_eq!(
                    out.completed_slices,
                    d["completed"].as_u64().unwrap() as usize
                );
                optional_close(out.phase_fraction, &d["fraction"]);
                assert_eq!(
                    out.weights_estimated,
                    d["weights_estimated"].as_bool().unwrap()
                );
                let c = &d["cost"];
                for (actual, want) in [
                    out.cost.so_far,
                    out.cost.projected,
                    out.cost.at_limit,
                    out.cost.gpu_hours,
                ]
                .into_iter()
                .zip([&c[0], &c[1], &c[3], &c[4]])
                {
                    optional_close(actual, want);
                }
                match (out.cost.projected_range, c[2].as_array()) {
                    (None, None) => (),
                    (Some(r), Some(want)) => {
                        close(r[0], want[0].as_f64().unwrap());
                        close(r[1], want[1].as_f64().unwrap());
                    }
                    other => panic!("{} {other:?}", moment.name),
                }
                assert_eq!(out.cost.spend, moment.spend);
                close(out.cost.at, i.now);
                assert_eq!(out.cost.uncertain, c[5].as_bool().unwrap());
                match moment.expected {
                    None => assert!(out.estimate.is_none()),
                    Some(expected) if expected.get("pending").is_some() => {
                        assert!(out.estimate.unwrap().pending)
                    }
                    Some(expected) => {
                        let r = out.estimate.unwrap().range.unwrap();
                        for (key, actual) in [("lo", r.lo_ms), ("mid", r.mid_ms), ("hi", r.hi_ms)] {
                            close(actual, expected[key].as_f64().unwrap() * 1000.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_reports_use_the_phase_start_only_for_prior_silence() {
        let mut i = input(Some(3.0));
        i.points.clear();
        i.phase_started_at = Some(50_000.0);
        i.now = 350_000.0;
        i.as_of = i.now;
        let out = estimate(&i).unwrap();
        assert!(out.estimate.unwrap().pending);
        assert_eq!(out.whole, None);
        assert_eq!(out.stall_ms, None);
        i.phases[0].prior = Some(Range {
            lo_ms: 3_600_000.0,
            mid_ms: 3_600_000.0,
            hi_ms: 3_600_000.0,
        });
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
        i.now = 50_000.0 + 4_800_001.0;
        i.as_of = i.now;
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(4_800_001.0));
    }

    #[test]
    fn reviewed_resume_inputs_have_no_synthetic_origin() {
        let mut i = input(Some(5000.0));
        i.points = vec![Point {
            at: 2000.0,
            done: 3001.0,
        }];
        assert_eq!(estimate(&i).unwrap().stall_ms, None);
        i.phases[0].prior = Some(Range {
            lo_ms: 10_000_000.0,
            mid_ms: 10_000_000.0,
            hi_ms: 10_000_000.0,
        });
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
        i.phases[0].prior = None;
        i.points = (0..=4)
            .map(|k| Point {
                at: 2000.0 + k as f64 * 100_000.0,
                done: 3000.0 + k as f64 * 100.0,
            })
            .collect();
        i.now = 402_000.0;
        i.as_of = i.now;
        let out = estimate(&i).unwrap();
        assert_eq!(out.completed_slices, 3);
        close(out.estimate.unwrap().range.unwrap().mid_ms, 1_600_000.0);
    }

    #[test]
    fn off_boundary_resume_does_not_time_the_partial_slice() {
        let mut i = input(Some(5000.0));
        i.points = (0..=140)
            .map(|k| Point {
                at: k as f64 * 1000.0,
                done: 3120.0 + k as f64,
            })
            .collect();
        i.now = 140_000.0;
        i.as_of = i.now;
        let out = estimate(&i).unwrap();
        assert_eq!(out.completed_slices, 1);
        let r = out.estimate.unwrap().range.unwrap();
        close(r.mid_ms, 1_740_000.0);
        assert!(r.lo_ms <= 1_740_000.0 && r.hi_ms >= 1_740_000.0);
        i.phases[0].prior = Some(Range {
            lo_ms: 10_000_000.0,
            mid_ms: 10_000_000.0,
            hi_ms: 10_000_000.0,
        });
        let prior = estimate(&i).unwrap();
        assert_eq!(prior.completed_slices, 1);
        assert!(prior.estimate.unwrap().range.unwrap().mid_ms > r.mid_ms);
        for seconds in [270, 395] {
            i.points = (0..=seconds)
                .map(|k| Point {
                    at: k as f64 * 1000.0,
                    done: 3120.0 + k as f64,
                })
                .collect();
            i.now = seconds as f64 * 1000.0;
            i.as_of = i.now;
            let out = estimate(&i).unwrap();
            let n = out.completed_slices;
            assert!(n < 4);
            let remaining = (5000.0 - i.points.last().unwrap().done) * 1000.0;
            close(
                out.estimate.unwrap().range.unwrap().mid_ms,
                remaining * (1.0 + (4 - n) as f64 / 4.0),
            );
        }
    }

    #[test]
    fn partial_observation_is_followed_by_a_full_warmup_slice() {
        for (first, offset) in [(0.5, 10_000.0), (1.0, 20_000.0)] {
            let mut i = input(Some(40.0));
            i.points = vec![
                Point {
                    at: offset,
                    done: first,
                },
                Point {
                    at: 20_000.0,
                    done: 1.0,
                },
                Point {
                    at: 220_000.0,
                    done: 2.0,
                },
                Point {
                    at: 320_000.0,
                    done: 3.0,
                },
                Point {
                    at: 370_000.0,
                    done: 3.5,
                },
            ];
            if first == 1.0 {
                i.points.remove(1);
            }
            i.now = 370_000.0;
            i.as_of = i.now;
            let out = estimate(&i).unwrap();
            assert_eq!(out.completed_slices, 2);
            close(out.estimate.unwrap().range.unwrap().mid_ms, 3_650_000.0);
        }
    }

    #[test]
    fn small_and_fractional_totals_use_only_observed_slices() {
        for (total, first, done, n) in [
            (3.0, 0.5, 1.5, 0),
            (2.5, 0.5, 1.8, 1),
            (0.5, 0.1, 0.25, 0),
            (24.0, 1.0, 4.0, 3),
        ] {
            let mut i = input(Some(total));
            i.points = vec![
                Point {
                    at: 0.0,
                    done: first,
                },
                Point {
                    at: 100_000.0,
                    done,
                },
            ];
            let out = estimate(&i).unwrap();
            assert_eq!(out.completed_slices, n);
            close(out.phase_fraction.unwrap(), done / total);
            assert!(out.estimate.unwrap().range.unwrap().mid_ms > 0.0);
        }
    }

    #[test]
    fn same_timestamp_reports_cannot_supply_slice_timing() {
        let mut i = input(Some(100.0));
        i.points = vec![
            Point {
                at: 100_000.0,
                done: 5.0,
            },
            Point {
                at: 100_000.0,
                done: 10.0,
            },
        ];
        let out = estimate(&i).unwrap();
        assert_eq!(out.completed_slices, 0);
        assert!(out.estimate.unwrap().pending);
        i.phases[0].prior = Some(Range {
            lo_ms: 100.0,
            mid_ms: 200.0,
            hi_ms: 400.0,
        });
        assert!(!estimate(&i).unwrap().estimate.unwrap().pending);
    }

    #[test]
    fn alternating_pauses_and_slowing_rounds() {
        let durations: Vec<_> = (1..=20)
            .map(|k| if k % 2 == 0 { 30_000.0 } else { 10_000.0 })
            .collect();
        let i = slice_reports(&durations);
        close(
            estimate(&i)
                .unwrap()
                .estimate
                .unwrap()
                .range
                .unwrap()
                .mid_ms,
            380_000.0,
        );
        let durations: Vec<_> = (1..=20).map(|k| 10_000.0 + k as f64 * 1000.0).collect();
        let i = slice_reports(&durations);
        close(
            estimate(&i)
                .unwrap()
                .estimate
                .unwrap()
                .range
                .unwrap()
                .mid_ms,
            (31..=49).sum::<u32>() as f64 * 1000.0,
        );
    }

    #[test]
    fn future_phase_priors_and_estimated_weights() {
        let mut i = input(Some(100.0));
        i.phases.push(Phase {
            name: "eval".into(),
            total: Some(5.0),
            prior: None,
            spend: None,
        });
        let out = estimate(&i).unwrap();
        assert!(out.estimate.unwrap().pending);
        assert!(!out.current_phase.unwrap().pending);
        assert!(out.weights_estimated);
        close(out.whole.unwrap(), 0.05);
        i.phases[1].prior = Some(Range {
            lo_ms: 100_000.0,
            mid_ms: 100_000.0,
            hi_ms: 100_000.0,
        });
        let out = estimate(&i).unwrap();
        let cur = out.current_phase.unwrap().range.unwrap();
        let all = out.estimate.unwrap().range.unwrap();
        close(all.lo_ms - cur.lo_ms, 85_000.0);
        close(all.hi_ms - cur.hi_ms, 120_000.0);
        i.phases[1].prior = Some(Range {
            lo_ms: 50_000.0,
            mid_ms: 100_000.0,
            hi_ms: 200_000.0,
        });
        let out = estimate(&i).unwrap();
        let cur = out.current_phase.unwrap().range.unwrap();
        let all = out.estimate.unwrap().range.unwrap();
        close(all.lo_ms - cur.lo_ms, 50_000.0);
        close(all.hi_ms - cur.hi_ms, 200_000.0);
        i.points.push(Point {
            at: 110_000.0,
            done: 100.0,
        });
        i.now = 110_000.0;
        i.as_of = i.now;
        let out = estimate(&i).unwrap();
        assert_eq!(out.stall_ms, Some(0.0));
        close(out.current_phase.unwrap().range.unwrap().mid_ms, 0.0);
        assert!(out.estimate.unwrap().range.unwrap().mid_ms > 0.0);
    }

    #[test]
    fn open_ended_progress_uses_elapsed_not_reports() {
        let mut i = input(None);
        i.points.clear();
        assert_eq!(estimate(&i).unwrap().whole, None);
        i.ends_at = Some(200_000.0);
        i.machine = priced();
        let out = estimate(&i).unwrap();
        close(out.whole.unwrap(), 0.5);
        close(out.segments[0].fill.unwrap(), 0.5);
        assert!(out.estimate.is_none());
        close(out.cost.at_limit.unwrap(), 200.0);
        i.spend = Spend::Reported {
            usd: 10.0,
            at_phase_start: 0.0,
        };
        assert_eq!(estimate(&i).unwrap().cost.at_limit, None);
        i.spend_per_hour = Some(360.0);
        close(estimate(&i).unwrap().cost.at_limit.unwrap(), 220.0);
    }

    #[test]
    fn stall_ignores_heartbeats_and_contact_loss_freezes_cost() {
        let mut i = input(Some(100.0));
        i.machine = priced();
        let baseline = estimate(&i).unwrap();
        i.now = 200_000.0;
        i.as_of = i.now;
        i.points.push(Point {
            at: i.now,
            done: 10.0,
        });
        let stalled = estimate(&i).unwrap();
        assert_eq!(stalled.stall_ms, Some(100_000.0));
        close(stalled.estimate.as_ref().unwrap().at, 100_000.0);
        close(
            stalled.estimate.unwrap().range.unwrap().mid_ms,
            baseline.estimate.unwrap().range.unwrap().mid_ms,
        );
        i.contact_lost = true;
        i.now = 5_000_000.0;
        let lost = estimate(&i).unwrap();
        close(lost.cost.at, 200_000.0);
        close(lost.cost.so_far.unwrap(), 200.0);
        close(lost.cost.gpu_hours.unwrap(), 1.0 / 9.0);
        assert!(lost.cost.uncertain);
    }

    #[test]
    fn deadline_and_cost_use_caller_time() {
        let mut i = slice_reports(&[60_000.0; 10]);
        i.machine = priced();
        i.now += 45.0 * 60_000.0;
        i.as_of = i.now;
        i.ends_at = Some(i.now + 300_000.0);
        let out = estimate(&i).unwrap();
        let eta = out.estimate.unwrap();
        let r = eta.range.unwrap();
        close(
            eta.past_limit_ms.unwrap(),
            i.now + r.hi_ms - i.ends_at.unwrap(),
        );
        close(out.cost.projected.unwrap(), (i.now + r.mid_ms) / 1000.0);
    }

    #[test]
    fn buffered_reports_do_not_count_unobserved_work() {
        let mut i = input(Some(1000.0));
        i.points.clear();
        for burst in 1..=5 {
            for step in 1..=100 {
                i.points.push(Point {
                    at: burst as f64 * 60_000.0,
                    done: ((burst - 1) * 100 + step) as f64,
                });
            }
        }
        i.now = 300_000.0;
        i.as_of = i.now;
        let out = estimate(&i).unwrap();
        assert_eq!(out.completed_slices, 16);
        close(out.estimate.unwrap().range.unwrap().mid_ms, 300_000.0);
    }

    #[test]
    fn prediction_floor_and_width_ceiling_bind() {
        let durations: Vec<_> = (1..=20).map(|k| (30 - k) as f64 * 1000.0).collect();
        let i = slice_reports(&durations);
        close(
            estimate(&i)
                .unwrap()
                .estimate
                .unwrap()
                .range
                .unwrap()
                .mid_ms,
            104_300.0,
        );
        let i = slice_reports(&[50_000.0, 1000.0, 199_000.0]);
        let r = estimate(&i).unwrap().estimate.unwrap().range.unwrap();
        close(r.mid_ms, 3_600_000.0);
        close(r.lo_ms, 1_440_000.0);
        close(r.hi_ms, 5_760_000.0);
    }

    #[test]
    fn cadence_floor_multiplier_and_window_bind() {
        let mut i = slice_reports(&[1000.0, 1000.0]);
        i.now += 80_000.0;
        i.as_of = i.now;
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
        let mut i = slice_reports(&[100_000.0, 100_000.0]);
        i.now += 350_000.0;
        i.as_of = i.now;
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
        i.now += 100_000.0;
        i.as_of = i.now;
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(450_000.0));
        let mut i = slice_reports(&[vec![5000.0], vec![100_000.0; 6], vec![10_000.0; 6]].concat());
        i.now += 150_000.0;
        i.as_of = i.now;
        assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
    }

    #[test]
    fn narrow_prior_is_widened_without_reports() {
        let mut i = input(Some(40.0));
        i.points.clear();
        i.phases[0].prior = Some(Range {
            lo_ms: 90_000.0,
            mid_ms: 100_000.0,
            hi_ms: 110_000.0,
        });
        let r = estimate(&i).unwrap().current_phase.unwrap().range.unwrap();
        close(r.lo_ms, 75_000.0);
        close(r.hi_ms, 130_000.0);
    }

    #[test]
    fn tiny_totals_and_completed_reports_do_not_invent_slices() {
        for total in [3.0, 5e-10] {
            let mut i = input(Some(total));
            i.points = vec![Point {
                at: 100_000.0,
                done: total,
            }];
            let out = estimate(&i).unwrap();
            assert_eq!(out.completed_slices, 0);
            assert_eq!(out.stall_ms, Some(0.0));
            close(out.estimate.unwrap().range.unwrap().mid_ms, 0.0);
            i.points[0].done = total * 0.99;
            let out = estimate(&i).unwrap();
            assert_eq!(out.completed_slices, 0);
            assert!(out.estimate.unwrap().pending);
        }
    }

    #[test]
    fn machine_and_api_costs_keep_quantum_and_unknowns() {
        let mut i = input(Some(100.0));
        i.machine = Machine::Priced {
            usd_per_hour: 3600.0,
            quantum_ms: 60_000.0,
            billed_from: Some(0.0),
            uncertain: false,
        };
        i.spend = Spend::Reported {
            usd: 10.0,
            at_phase_start: 0.0,
        };
        i.ends_at = Some(2_000_000.0);
        let out = estimate(&i).unwrap();
        close(out.cost.so_far.unwrap(), 130.0);
        close(out.cost.projected.unwrap(), 1120.0);
        close(out.cost.at_limit.unwrap(), 2140.0);
        let bounds = out.cost.projected_range.unwrap();
        assert!(bounds[0] <= 1120.0 && bounds[1] >= 1120.0);
        i.machine = Machine::Unknown;
        let out = estimate(&i).unwrap();
        assert_eq!(out.cost.so_far, None);
        assert_eq!(out.cost.spend, Some(10.0));
        assert!(out.cost.uncertain);
        i.machine = Machine::Priced {
            usd_per_hour: 1.0,
            quantum_ms: 1.0,
            billed_from: None,
            uncertain: true,
        };
        assert_eq!(estimate(&i).unwrap().cost.projected, None);
    }

    #[test]
    fn spend_priors_and_unknown_future_spend() {
        let mut i = input(Some(100.0));
        i.phases[0].spend = Some(20.0);
        i.phases.push(Phase {
            name: "judge".into(),
            total: Some(10.0),
            prior: Some(Range {
                lo_ms: 2000.0,
                mid_ms: 2000.0,
                hi_ms: 2000.0,
            }),
            spend: Some(5.0),
        });
        let out = estimate(&i).unwrap();
        close(out.cost.projected.unwrap(), 25.0);
        assert_eq!(out.cost.so_far, None);
        i.spend = Spend::Reported {
            usd: 10.0,
            at_phase_start: 5.0,
        };
        close(estimate(&i).unwrap().cost.projected.unwrap(), 60.0);
        i.phases[1].spend = None;
        let out = estimate(&i).unwrap();
        assert_eq!(out.cost.projected, None);
        assert!(out.cost.uncertain);
        i.phases[1].spend = Some(20.0);
        i.phases[0].total = None;
        i.ends_at = Some(i.now + 3_600_000.0);
        i.spend_per_hour = Some(2.0);
        close(estimate(&i).unwrap().cost.at_limit.unwrap(), 32.0);
    }

    #[test]
    fn negative_phase_spend_is_disabled_and_reported() {
        let mut i = input(Some(100.0));
        i.phases[0].spend = Some(-20.0);
        let out = estimate(&i).unwrap();
        assert!(out.errors.contains(&InputError::BelowMinimum {
            field: "phase.spend",
            index: Some(0),
            value: -20.0,
            minimum: 0.0
        }));
        assert_eq!(out.cost.projected, None);
        assert!(out.cost.uncertain);
        assert!(!out.estimate.unwrap().pending);
        i.phases[0].spend = None;
        i.phases.push(Phase {
            name: "judge".into(),
            total: Some(10.0),
            prior: None,
            spend: Some(-5.0),
        });
        i.spend = Spend::Reported {
            usd: 10.0,
            at_phase_start: 0.0,
        };
        assert_eq!(estimate(&i).unwrap().cost.projected, None);
    }

    #[test]
    fn spend_projection_is_independent_of_timing() {
        let mut i = input(Some(100.0));
        i.points = vec![Point {
            at: 100_000.0,
            done: 10.0,
        }];
        i.spend = Spend::Reported {
            usd: 10.0,
            at_phase_start: 0.0,
        };
        let out = estimate(&i).unwrap();
        assert!(out.estimate.unwrap().pending);
        close(out.cost.projected.unwrap(), 100.0);
    }

    #[test]
    fn independent_components_keep_all_degradation_reasons() {
        let mut i = input(Some(100.0));
        i.started_at = i.now + 5001.0;
        i.machine = Machine::Priced {
            usd_per_hour: 1.0,
            quantum_ms: 1.0,
            billed_from: Some(i.now + 5001.0),
            uncertain: false,
        };
        i.points[5].at = 1000.0;
        i.phases[0].prior = Some(Range {
            lo_ms: 200.0,
            mid_ms: 100.0,
            hi_ms: 300.0,
        });
        let out = estimate(&i).unwrap();
        assert!(out.estimate.unwrap().pending);
        close(out.whole.unwrap(), 0.1);
        for field in ["started_at", "billed_from", "points.at", "prior.lo_ms"] {
            assert!(
                out.errors
                    .iter()
                    .any(|error| error.to_string().contains(field)),
                "{field}: {:?}",
                out.errors
            );
        }
        assert_eq!(out.cost.gpu_hours, None);
        assert_eq!(out.cost.so_far, None);
    }

    #[test]
    fn five_second_clock_skew_clamps_without_masking_timing_errors() {
        let mut i = input(Some(100.0));
        i.spend = Spend::Reported {
            usd: 0.0,
            at_phase_start: 0.0,
        };
        for skew in [3.0, 5000.0] {
            i.started_at = i.now + skew;
            i.machine = Machine::Priced {
                usd_per_hour: 1.0,
                quantum_ms: 1.0,
                billed_from: Some(i.now + skew),
                uncertain: false,
            };
            let out = estimate(&i).unwrap();
            assert!(out.errors.is_empty());
            assert!(!out.cost.uncertain);
            assert_eq!(out.cost.gpu_hours, Some(0.0));
            assert_eq!(out.cost.so_far, Some(0.0));
        }
        i.points[5].at = 1000.0;
        let out = estimate(&i).unwrap();
        assert_eq!(out.errors.len(), 1);
        assert!(matches!(
            out.errors[0],
            InputError::BelowMinimum {
                field: "points.at",
                index: Some(5),
                ..
            }
        ));
        assert!(out.estimate.unwrap().pending);
    }

    #[test]
    fn malformed_points_keep_progress_and_observed_billing() {
        let mut i = input(Some(100.0));
        i.machine = priced();
        i.points[9].at = 200_000.0;
        let out = estimate(&i).unwrap();
        assert!(out.errors.contains(&InputError::AboveMaximum {
            field: "points.at",
            index: Some(9),
            value: 200_000.0,
            maximum: 100_000.0
        }));
        assert!(out.estimate.unwrap().pending);
        close(out.whole.unwrap(), 0.1);
        close(out.cost.so_far.unwrap(), 100.0);
        i.points[9].at = 100_000.0;
        i.phases[0].total = Some(0.0);
        let out = estimate(&i).unwrap();
        assert!(out.estimate.unwrap().pending);
        assert_eq!(out.whole, None);
        close(out.cost.so_far.unwrap(), 100.0);
        i.now = f64::NAN;
        assert!(matches!(
            estimate(&i),
            Err(InputError::NonFinite { field: "now", .. })
        ));
    }

    #[test]
    fn bad_prior_baseline_and_later_total_degrade_independently() {
        let mut i = input(Some(100.0));
        i.machine = priced();
        i.phases[0].prior = Some(Range {
            lo_ms: 200.0,
            mid_ms: 100.0,
            hi_ms: 300.0,
        });
        i.spend = Spend::Reported {
            usd: 5.0,
            at_phase_start: 10.0,
        };
        i.phases.push(Phase {
            name: "eval".into(),
            total: Some(0.0),
            prior: Some(Range {
                lo_ms: 100_000.0,
                mid_ms: 100_000.0,
                hi_ms: 100_000.0,
            }),
            spend: Some(0.0),
        });
        let out = estimate(&i).unwrap();
        assert_eq!(out.errors.len(), 3);
        assert!(out.weights_estimated);
        close(out.current_phase.unwrap().range.unwrap().mid_ms, 900_000.0);
        close(out.estimate.unwrap().range.unwrap().mid_ms, 1_000_000.0);
        close(out.cost.so_far.unwrap(), 105.0);
        assert_eq!(out.cost.projected, None);
    }

    #[test]
    fn error_display_names_field_index_and_value() {
        let error = InputError::AboveMaximum {
            field: "points.at",
            index: Some(2),
            value: 20.0,
            maximum: 10.0,
        };
        let message = error.to_string();
        assert!(message.contains("points.at"));
        assert!(message.contains("[2]"));
        assert!(message.contains("20"));
        let _: &dyn std::error::Error = &error;
    }
    #[test]
    fn an_observed_zero_report_is_not_an_invented_origin() {
        let mut i = input(Some(40.0));
        i.points = vec![
            Point {
                at: 10_000.0,
                done: 0.0,
            },
            Point {
                at: 20_000.0,
                done: 1.0,
            },
            Point {
                at: 30_000.0,
                done: 2.0,
            },
        ];
        let out = estimate(&i).unwrap();
        assert_eq!(out.completed_slices, 2);
        close(out.estimate.unwrap().range.unwrap().mid_ms, 380_000.0);
    }

    #[test]
    fn zero_report_changes_evidence_count_not_the_settled_rate() {
        let mut with_zero = input(Some(40.0));
        with_zero.phases[0].prior = Some(Range {
            lo_ms: 400_000.0,
            mid_ms: 400_000.0,
            hi_ms: 400_000.0,
        });
        with_zero.points = vec![Point { at: 0.0, done: 0.0 }];
        let mut at = 0.0;
        for step in 1..=30 {
            at += match step {
                1 => 200_000.0,
                2 => 100_000.0,
                _ => 10_000.0,
            };
            with_zero.points.push(Point {
                at,
                done: step as f64,
            });
        }
        let reports = with_zero.points.clone();
        for step in 1..=30 {
            with_zero.points = reports[..=step].to_vec();
            with_zero.now = reports[step].at;
            with_zero.as_of = with_zero.now;
            let mut without_zero = with_zero.clone();
            without_zero.points.remove(0);
            let a = estimate(&with_zero).unwrap();
            let b = estimate(&without_zero).unwrap();
            assert_eq!(a.completed_slices, b.completed_slices + 1);
            let a = a.estimate.unwrap().range.unwrap();
            let b = b.estimate.unwrap().range.unwrap();
            if step == 2 {
                close(a.mid_ms, 2_090_000.0);
                close(b.mid_ms, 380_000.0);
            }
            if step >= 17 {
                close(a.mid_ms, (40 - step) as f64 * 10_000.0);
                close(a.mid_ms, b.mid_ms);
                let evidence_width =
                    a.mid_ms * 0.35 * (1.0 / step as f64 - 1.0 / (step + 1) as f64);
                close(a.lo_ms - b.lo_ms, evidence_width);
                close(b.hi_ms - a.hi_ms, evidence_width);
            }
        }
    }

    #[test]
    fn cadence_starts_after_the_observation_boundary_for_zero_and_resumed_reports() {
        for (first, total) in [(0.0, 1000.0), (3000.0, 5000.0)] {
            let mut i = input(Some(total));
            i.points = vec![
                Point {
                    at: 0.0,
                    done: first,
                },
                Point {
                    at: 2000.0,
                    done: first + 1.0,
                },
            ];
            assert_eq!(estimate(&i).unwrap().stall_ms, None);
            i.phases[0].prior = Some(Range {
                lo_ms: 4_000_000.0,
                mid_ms: 4_000_000.0,
                hi_ms: 4_000_000.0,
            });
            assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
            i.phases[0].prior = None;
            i.points.push(Point {
                at: 102_000.0,
                done: first + 2.0,
            });
            i.now = 452_000.0;
            i.as_of = i.now;
            assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
            i.now = 552_000.0;
            i.as_of = i.now;
            assert_eq!(estimate(&i).unwrap().stall_ms, Some(450_000.0));
            i.points[1].at = 0.0;
            i.points[2].at = 100_000.0;
            i.now = 450_000.0;
            i.as_of = i.now;
            assert_eq!(estimate(&i).unwrap().stall_ms, Some(0.0));
            i.now = 550_000.0;
            i.as_of = i.now;
            assert_eq!(estimate(&i).unwrap().stall_ms, Some(450_000.0));
        }
    }

    #[test]
    fn fractional_width_preserves_the_first_observed_boundary() {
        for (total, first) in [(3.3, 2.475), (3.2, 2.4)] {
            let mut i = input(Some(total));
            i.points = vec![
                Point {
                    at: 10_000.0,
                    done: first,
                },
                Point {
                    at: 100_000.0,
                    done: total,
                },
            ];
            assert_eq!(estimate(&i).unwrap().completed_slices, 1);
        }
        let mut i = input(Some(5e-10));
        i.points = vec![
            Point {
                at: 10_000.0,
                done: 1e-20,
            },
            Point {
                at: 100_000.0,
                done: 5e-10,
            },
        ];
        assert_eq!(estimate(&i).unwrap().completed_slices, 0);
    }
}

#[cfg(test)]
mod retained_timing {
    use super::*;

    /// Each line arrives at its time in ms; lines sharing a time are one read.
    fn agrees(name: &str, lines: &[(i64, String)], every_line_reports: bool) {
        let started = lines[0].0 as f64;
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        let mut all = Vec::new();
        let mut stalled = 0;
        let mut ranged = 0;
        let mut reads = 0;
        for (index, (at, text)) in lines.iter().enumerate() {
            let stretch = |s: &orx_signals::Snapshot| {
                let origins = s.diagnostics.counts.get("t_origin").copied();
                (s.phases.last().map(|p| (p.occurrence, p.of)), origins)
            };
            let before = stretch(&signals);
            let clock = signals.timing.payload_t;
            let last = signals.timing.progress_points.last().copied();
            signals
                .feed(format!("{text}\n").as_bytes(), false, *at, true)
                .unwrap();
            // A clock that restarted is a new stretch whether or not it was diagnosed.
            if stretch(&signals) != before || signals.timing.payload_t < clock {
                all.clear();
            }
            let newest = signals.timing.progress_points.last().copied();
            if newest.is_some() && (every_line_reports || newest != last) {
                all.extend(newest);
            }
            if signals.phases.is_empty() || lines.get(index + 1).is_some_and(|next| next.0 == *at) {
                continue;
            }
            signals.reached_end(*at);

            let mut every = signals.clone();
            every.timing.progress_points = all.clone();
            reads += 1;
            let span = if reads % 7 == 0 { 600_000 } else { 0 };
            for later in (0..=span).step_by(15_000) {
                let now = (*at + later) as f64;
                let run = Run {
                    ends_at: Some(started + 6.0 * 3_600_000.0),
                    ..run(started)
                };
                let clock = Some((*at as f64, *at as f64));
                let kept = estimate(&with_signals(run.clone(), &signals, &[], clock, now)).unwrap();
                let whole = estimate(&with_signals(run, &every, &[], clock, now)).unwrap();
                assert_eq!(
                    format!("{kept:?}"),
                    format!("{whole:?}"),
                    "{name}: line {index}, {later} ms later"
                );
                stalled += usize::from(kept.stall_ms.is_some_and(|ms| ms > 0.0));
                ranged += usize::from(kept.current_phase.is_some_and(|eta| eta.range.is_some()));
            }
        }
        let occurrence = signals.phases.last().unwrap().occurrence;
        let kept = signals
            .timing
            .progress_points
            .iter()
            .filter(|p| p.0 == occurrence)
            .count();
        assert!(kept < all.len(), "{name} kept all {kept} reports");
        assert!(stalled > 0, "{name} never stalled");
        assert!(ranged > 0 || name == "open bar", "{name} never had a range");
    }

    fn declared(step: f64, of: f64, t: Option<f64>) -> String {
        let t = t.map_or(String::new(), |t| format!(",\"t\":{t}"));
        format!(
            "::orx:: {{\"step\":{step},\"of\":{of}{t},\"m\":{{\"loss\":{}}}}}",
            1.0 / (step + 1.0)
        )
    }

    fn run(started_at: f64) -> Run {
        Run {
            started_at,
            contact_lost: false,
            ends_at: None,
            machine: Machine::Unpriced,
            gpu_count: None,
            spend: Spend::NeverReported,
            spend_per_hour: None,
        }
    }

    /// A first scan reads every report after the fact, but each was written no later than the
    /// log last changed: a run that hung an hour before anyone looked shows an hour of silence.
    #[test]
    fn a_run_that_hung_before_the_first_scan_shows_its_silence() {
        let dir = crate::local::git::TemporaryDirectory::new("orx-first-scan").unwrap();
        let log = dir.path().join("log");
        let lines: String = (0..=6)
            .map(|step| {
                format!(
                    "::orx:: {{\"step\":{step},\"of\":10,\"t\":{}}}\n",
                    step * 10
                )
            })
            .collect();
        std::fs::write(&log, lines).unwrap();
        let started = 1_794_000_000_000_i64;
        let hung = started + 75_000;
        let at = |ms: i64| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
        std::fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_modified(at(hung))
            .unwrap();
        let read = hung + 3_600_000;
        let signals = crate::engine::adapters::local::signals::scan(
            &log,
            &dir.path().join("r1.json"),
            "r1",
            false,
            read,
            Some(started),
        )
        .unwrap()
        .unwrap();
        let clock = Some((read as f64, read as f64));
        let input = with_signals(run(started as f64), &signals, &[], clock, read as f64);
        assert_eq!(input.phase_started_at, Some(started as f64));
        assert!(input.placed);
        let last = input.points.last().unwrap();
        assert_eq!(
            last.at, hung as f64,
            "the last advance sits when the log last changed"
        );
        assert_eq!(input.points[0].at, (hung - 60_000) as f64);
        let out = estimate(&input).unwrap();
        assert_eq!(out.stall_ms, Some(3_600_000.0));
    }

    /// `t` counts from wherever the payload chose, after imports and loading: the time before
    /// its first report changes nothing about a healthy run.
    #[test]
    fn setup_before_the_first_report_changes_nothing() {
        let estimates: Vec<_> = [0, 30, 95, 600]
            .into_iter()
            .map(|setup| {
                let started = 1_794_000_000_000_i64;
                let origin = started + setup * 1000;
                let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
                signals.timing.started_ms = Some(started);
                let mut seen = Vec::new();
                for second in 0..=300_i64 {
                    let line =
                        format!("::orx:: {{\"step\":{second},\"of\":1000,\"t\":{second}}}\n");
                    let written = origin + second * 1000;
                    let now = written + 400;
                    signals.feed(line.as_bytes(), false, now, true).unwrap();
                    signals.reached_end(written);
                    if second % 5 == 0 && second > 0 {
                        let clock = Some((now as f64, now as f64));
                        let out = estimate(&with_signals(
                            run(started as f64),
                            &signals,
                            &[],
                            clock,
                            now as f64,
                        ))
                        .unwrap();
                        assert_eq!(out.stall_ms, Some(0.0), "setup {setup}s, report {second}");
                        let range = out.current_phase.unwrap().range.map(|r| r.mid_ms.round());
                        seen.push((range, out.completed_slices, out.phase_fraction));
                    }
                }
                seen
            })
            .collect();
        assert!(estimates.windows(2).all(|pair| pair[0] == pair[1]));
    }

    /// Times in a checkpoint are the host's; the laptop's clock only measures time since it
    /// received it, so skew of any size gives the same estimate.
    #[test]
    fn host_clock_skew_changes_nothing() {
        let laptop_started = 1_794_000_000_000_f64;
        let estimates: Vec<_> = [
            0,
            10_000,
            -10_000,
            100_000,
            -100_000,
            86_400_000,
            -86_400_000,
        ]
        .into_iter()
        .map(|skew: i64| {
            let host = |laptop: f64| laptop as i64 + skew;
            let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
            signals.timing.started_ms = Some(host(laptop_started));
            for second in 0..=60 {
                let line = format!("::orx:: {{\"step\":{second},\"of\":100,\"t\":{second}}}\n");
                let read = laptop_started + second as f64 * 1000.0;
                signals
                    .feed(line.as_bytes(), false, host(read), true)
                    .unwrap();
                signals.reached_end(host(read));
            }
            let received = laptop_started + 61_000.0;
            let clock = Some((host(received) as f64, received));
            let run = Run {
                ends_at: Some(laptop_started + 120_000.0),
                machine: Machine::Priced {
                    usd_per_hour: 3.6,
                    quantum_ms: 1_000.0,
                    billed_from: Some(laptop_started - 30_000.0),
                    uncertain: false,
                },
                ..run(laptop_started)
            };
            [0.0, 30_000.0, 200_000.0].map(|later| {
                let out = estimate(&with_signals(
                    run.clone(),
                    &signals,
                    &[],
                    clock,
                    received + later,
                ))
                .unwrap();
                assert!(out.errors.is_empty(), "skew {skew}: {:?}", out.errors);
                let eta = out.current_phase.unwrap();
                (
                    out.stall_ms,
                    eta.range.map(|r| r.mid_ms.round()),
                    (eta.at - skew as f64).round(),
                    eta.past_limit_ms,
                    out.whole,
                    [out.cost.so_far, out.cost.at_limit, out.cost.projected]
                        .map(|usd| usd.map(|usd| (usd * 1e6).round())),
                )
            })
        })
        .collect();
        assert!(estimates[0][2].0.unwrap() > 0.0, "the last case stalls");
        assert!(
            estimates[0][2].3.unwrap() > 0.0,
            "the last case is past its limit"
        );
        assert!(estimates[0][0].5[1].is_some(), "a limit prices the run");
        assert!(
            estimates.windows(2).all(|pair| pair[0] == pair[1]),
            "{estimates:?}"
        );
    }

    /// The anchor is the last report that advanced, placed when a scan reached the end of the
    /// log: a later end with no new advance keeps it, a new advance moves it, and a later phase
    /// starts at its own first report.
    #[test]
    fn reports_are_placed_by_the_last_advance_at_the_end_of_the_log() {
        fn feed(signals: &mut orx_signals::Snapshot, line: &str, now: i64) {
            signals
                .feed(format!("::orx:: {line}\n").as_bytes(), false, now, true)
                .unwrap();
        }
        let at = |signals: &orx_signals::Snapshot, now: f64| {
            let input = with_signals(run(0.0), signals, &[], Some((now, now)), now);
            input.points.iter().map(|p| p.at).collect::<Vec<_>>()
        };
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        signals.timing.started_ms = Some(1_000);
        feed(
            &mut signals,
            r#"{"phase":"train","step":0,"of":10,"t":100}"#,
            200_000,
        );
        signals.reached_end(200_000);
        feed(&mut signals, r#"{"step":1,"t":110}"#, 210_500);
        signals.reached_end(210_000);
        feed(
            &mut signals,
            r#"{"step":1,"t":111,"m":{"loss":2}}"#,
            400_000,
        );
        signals.reached_end(399_000);
        assert_eq!(at(&signals, 400_000.0), [200_000.0, 210_000.0, 211_000.0]);
        let input = with_signals(
            run(0.0),
            &signals,
            &[],
            Some((400_000.0, 400_000.0)),
            400_000.0,
        );
        assert_eq!(input.phase_started_at, Some(1_000.0));

        feed(&mut signals, r#"{"step":2,"t":130}"#, 450_000);
        assert_eq!(
            at(&signals, 450_000.0)[..3],
            [200_000.0, 210_000.0, 211_000.0],
            "a pass that stops before the end moves nothing"
        );
        signals.reached_end(445_000);
        assert_eq!(
            at(&signals, 450_000.0),
            [415_000.0, 425_000.0, 426_000.0, 445_000.0],
            "the last advance, not the first, places the rest"
        );

        feed(
            &mut signals,
            r#"{"phase":"eval","step":0,"of":4,"t":500}"#,
            600_000,
        );
        feed(&mut signals, r#"{"step":1,"t":510}"#, 610_000);
        signals.reached_end(610_000);
        let input = with_signals(
            run(0.0),
            &signals,
            &[],
            Some((610_000.0, 610_000.0)),
            610_000.0,
        );
        assert_eq!(input.phase_started_at, Some(600_000.0));
        assert_eq!(
            input.phases[1].total,
            Some(4.0),
            "a declared phase keeps its total"
        );
    }

    /// The helper writes a step, then its metrics for the same step a moment later: placed from
    /// the advance, that report would lie past what the host knew, so it stops there.
    #[test]
    fn a_report_after_the_last_advance_lies_no_later_than_the_data() {
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        for step in 0..=5 {
            let line = format!("::orx:: {{\"step\":{step},\"of\":100,\"t\":{step}}}\n");
            signals.feed(line.as_bytes(), false, 9_000, true).unwrap();
        }
        let metrics = "::orx:: {\"step\":5,\"t\":5.5,\"m\":{\"loss\":1}}\n";
        signals
            .feed(metrics.as_bytes(), false, 9_000, true)
            .unwrap();
        signals.reached_end(9_000);
        let input = with_signals(run(0.0), &signals, &[], Some((9_000.0, 9_000.0)), 12_000.0);
        assert_eq!(input.points.last().unwrap().at, 9_000.0);
        let out = estimate(&input).unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
    }

    /// Reports not yet placed keep their durations, but their `t` is no time of day: the estimate
    /// is as of now.
    #[test]
    fn an_unplaced_replay_finishes_from_now() {
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        for step in 0..=20 {
            let line = format!(
                "::orx:: {{\"step\":{step},\"of\":40,\"t\":{}}}\n",
                step * 10
            );
            signals.feed(line.as_bytes(), false, 1_000, false).unwrap();
        }
        let now = 1_794_000_000_000.0;
        let input = with_signals(run(now - 600_000.0), &signals, &[], Some((now, now)), now);
        assert!(!input.placed);
        let eta = estimate(&input).unwrap().current_phase.unwrap();
        assert_eq!(eta.at, now);
        assert!(eta.range.is_some_and(|range| range.mid_ms > 0.0));
    }

    /// Without a pair, a checkpoint is read on the clock that wrote it: as of its own last time
    /// once it has ended, else against the laptop's now.
    #[test]
    fn a_checkpoint_without_a_pair_is_read_on_its_own_clock() {
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        signals
            .feed(b"::orx:: {\"step\":1,\"of\":4}\n", false, 50_000, true)
            .unwrap();
        let live = with_signals(run(0.0), &signals, &[], None, 80_000.0);
        assert_eq!((live.as_of, live.now), (50_000.0, 80_000.0));
        signals.feed(&[], true, 60_000, true).unwrap();
        let ended = with_signals(run(0.0), &signals, &[], None, 9_000_000.0);
        assert_eq!((ended.as_of, ended.now), (60_000.0, 60_000.0));
    }

    #[test]
    fn a_single_bar_is_the_whole_run_until_it_restarts() {
        let mut signals = orx_signals::Snapshot::new("r1", "1-1", "g1");
        let bar = |n: u32| format!("{n}/4 [00:0{n}<00:00, 1it/s]\r");
        let run = Run {
            ends_at: Some(40_000.0),
            ..run(0.0)
        };
        for n in 1..=3 {
            signals.feed(bar(n).as_bytes(), false, 3_000, true).unwrap();
        }
        let clock = Some((3_000.0, 3_000.0));
        let single = estimate(&with_signals(run.clone(), &signals, &[], clock, 10_000.0)).unwrap();
        assert_eq!(single.whole, Some(0.75));
        for n in [4, 1, 2] {
            signals.feed(bar(n).as_bytes(), false, 3_000, true).unwrap();
        }
        let several = estimate(&with_signals(run, &signals, &[], clock, 10_000.0)).unwrap();
        assert_eq!(several.whole, Some(0.25), "fills against the time limit");
    }

    #[test]
    fn blocks_without_t_read_once_a_minute() {
        let lines: Vec<_> = (0..=5000)
            .map(|step| {
                (
                    1_000_000 + (step / 40) * 60_000,
                    declared(step as f64, 5000.0, None),
                )
            })
            .collect();
        agrees("block-buffered", &lines, true);
    }

    #[test]
    fn coarse_t_shared_by_ten_reports() {
        let lines: Vec<_> = (0..=2000)
            .map(|step| {
                let t = (step / 10) as f64 * 15.0;
                (
                    1_000_000 + step * 1_500,
                    declared(step as f64, 2000.0, Some(t)),
                )
            })
            .collect();
        agrees("coarse t", &lines, true);
    }

    #[test]
    fn four_reports_per_step_at_uneven_pace() {
        let mut lines = Vec::new();
        let mut at = 1_000_000;
        for step in 0..300 {
            let pace = if step % 2 == 0 { 30_000 } else { 90_000 } + step * 7919 % 5 * 1_000;
            for report in 0..4 {
                let text = format!(
                    "::orx:: {{\"step\":{step},\"of\":300,\"m\":{{\"loss\":{}}}}}",
                    step * 4 + report
                );
                lines.push((at + report * pace / 4, text));
            }
            at += pace;
        }
        agrees("repeated steps", &lines, true);
    }

    #[test]
    fn resumed_run_and_changed_total() {
        // The helper's first record marks where observation starts; its next flush repeats it.
        let mut resumed = vec![
            (1_000_000, declared(3000.0, 5000.0, None)),
            (
                1_001_000,
                r#"::orx:: {"step":3000,"of":5000,"m":{"lr":0.1}}"#.into(),
            ),
        ];
        let mut at = 1_030_000;
        for read in 0..40 {
            resumed.extend(
                (1..=50).map(|i| (at, declared((3000 + read * 50 + i) as f64, 5000.0, None))),
            );
            at += if read % 2 == 0 { 30_000 } else { 90_000 };
        }
        agrees("resumed", &resumed, true);
        let changed: Vec<_> = (0..=6000)
            .map(|step| {
                let of = if step < 3000 { 5000.0 } else { 6000.0 };
                (1_000_000 + step * 1_000, declared(step as f64, of, None))
            })
            .collect();
        agrees("changed total", &changed, true);
    }

    /// A process resumed from an older checkpoint: `t` and the step both go back.
    #[test]
    fn relaunched_from_an_older_checkpoint() {
        let first = (0..=1200).map(|step| (step, step as f64 * 0.6));
        let second = (1000..=2000).map(|step| (step, (step - 1000) as f64 * 0.6 + 0.2));
        let lines: Vec<_> = first
            .chain(second)
            .enumerate()
            .map(|(index, (step, t))| {
                let at = 1_000_000 + index as i64 * 600 + i64::from(index > 1200) * 30_000;
                (at, declared(step as f64, 2000.0, Some(t)))
            })
            .collect();
        agrees("relaunched", &lines, true);
    }

    #[test]
    fn open_bar_redrawing_in_bursts() {
        let mut lines = Vec::new();
        let mut elapsed = 0;
        for burst in 0..600 {
            for redraw in 0..5 {
                let text = format!(
                    "{}it [{:02}:{:02}:{:02}, 0.20it/s]",
                    burst * 5 + redraw + 1,
                    elapsed / 3600,
                    elapsed / 60 % 60,
                    elapsed % 60
                );
                lines.push((1_000_000 + elapsed * 1_000 + redraw * 100, text));
            }
            elapsed += 10 + burst * 7919 % 61;
        }
        agrees("open bar", &lines, false);
    }
}
