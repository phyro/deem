//! `--stats`: what a run cost, on stderr, totalled from its events (the
//! same ones `--trace` writes).

use crate::trace::Event;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Default, Clone)]
pub struct Counts {
    pub records: u64,
    /// attempts sent to the API, retries included
    pub requests: u64,
    /// attempts that failed
    pub failures: u64,
    /// attempts retried
    pub throttles: u64,
    pub tokens: u64,
    pub questions: u64,
    pub cuts: u64,
    /// the deepest round any record sent: wave k is every record's k-th
    pub waves: u64,
    /// questions per wave, for the first `SIZES` waves
    pub wave_sizes: Vec<usize>,
    /// how long requests took, from sending to their answer, retries
    /// included: a count per `bucket`
    durations: Vec<u64>,
    /// the answers that are numbers: a count per fifth of [0,1]
    scores: [u64; 5],
    /// requests sent and not yet answered: number → when
    sent: HashMap<u64, u64>,
}

/// Wave sizes kept: enough for the report and for tests to read. The totals
/// don't grow with a run's length (TRANSPORT.md, rule 3).
const SIZES: usize = 100;
/// Duration buckets: whole milliseconds below 100 ms, then 2% wider each,
/// up to about 6 minutes.
const BUCKETS: usize = 512;

/// The bucket of a duration of `us` microseconds.
fn bucket(us: u64) -> usize {
    let ms = us / 1000;
    match ms {
        0..100 => ms as usize,
        _ => (100 + ((ms as f64 / 100.0).ln() / 1.02f64.ln()) as usize).min(BUCKETS - 1),
    }
}

/// The shortest duration in bucket `b`, in milliseconds.
fn floor_ms(b: usize) -> u64 {
    match b {
        0..100 => b as u64,
        _ => (100.0 * 1.02f64.powi((b - 100) as i32)).ceil() as u64,
    }
}

impl Counts {
    pub(crate) fn add(&mut self, t: u64, e: &Event) {
        match e {
            Event::Record { .. } => self.records += 1,
            Event::Cut { .. } => self.cuts += 1,
            Event::Wave { wave, questions, .. } => {
                self.waves = self.waves.max(*wave);
                let k = *wave as usize;
                if (1..=SIZES).contains(&k) {
                    if self.wave_sizes.len() < k {
                        self.wave_sizes.resize(k, 0);
                    }
                    self.wave_sizes[k - 1] += *questions;
                }
                self.questions += *questions as u64;
            }
            Event::Request { request, .. } => {
                self.sent.insert(*request, t);
            }
            Event::Answered { request, result, attempts, tokens, scores, .. } => {
                let failed = attempts - 1 + u64::from(result != "ok");
                (self.requests, self.throttles, self.failures) =
                    (self.requests + attempts, self.throttles + attempts - 1, self.failures + failed);
                self.tokens += tokens;
                let took = t - self.sent.remove(request).unwrap_or(t);
                self.durations.resize(BUCKETS, 0);
                self.durations[bucket(took)] += 1;
                for x in scores {
                    self.scores[((x * 5.0) as usize).min(4)] += 1;
                }
            }
            _ => {}
        }
    }

    /// The report; `elapsed` is the run's length in µs, `cap` the question
    /// limit.
    /// Requests that took less than `us` microseconds, to the nearest
    /// bucket.
    pub fn durations_under(&self, us: u64) -> u64 {
        self.durations.iter().enumerate().filter(|&(b, _)| floor_ms(b + 1) * 1000 <= us).map(|(_, n)| n).sum()
    }

    pub fn report(&self, elapsed: u64, cap: Option<u64>) -> String {
        // nearest rank: the smallest duration with a share `p` at or below
        // it, so the p99 of two requests is the slower one
        let total: u64 = self.durations.iter().sum();
        let pct = |p: f64| {
            let rank = ((total as f64 * p).ceil() as u64).max(1);
            let mut seen = 0;
            self.durations
                .iter()
                .position(|n| {
                    seen += n;
                    seen >= rank
                })
                .map_or(0, floor_ms)
        };
        let waves = match self.waves {
            1..=6 => format!(" ({} questions)", join(&self.wave_sizes)),
            _ => String::new(),
        };
        let cap = cap.map_or(" (no limit)".to_owned(), |n| format!(" of {n}"));
        let mut out = format!(
            "deem: records {} · questions {}{cap} · requests {} · waves {}{waves}\n",
            self.records, self.questions, self.requests, self.waves
        );
        let _ = writeln!(
            out,
            "deem: tokens {} · {:.1}s · p50 {}ms · p99 {}ms · failed {} · throttled {} · cut {}",
            human(self.tokens),
            elapsed as f64 / 1e6,
            pct(0.5),
            pct(0.99),
            self.failures,
            self.throttles,
            self.cuts
        );
        if self.records > 0 {
            let per = |x: u64| x as f64 * 1000.0 / self.records as f64;
            let _ = writeln!(out, "deem: per 1000 records: tokens {}", human(per(self.tokens) as u64));
        }
        let answers: u64 = self.scores.iter().sum();
        if answers > 0 {
            let _ = writeln!(out, "deem: score histogram ({answers} float answers)");
            out += &histogram(&self.scores);
        }
        out
    }
}

/// The cheap version of a calibration check: a question that doesn't
/// separate the corpus shows up here before any threshold is picked.
fn histogram(bins: &[u64; 5]) -> String {
    let max = bins.iter().copied().max().unwrap_or(1).max(1);
    let row =
        |(i, b): (usize, &u64)| format!("     {:.1} {} {b}\n", i as f64 / 5.0, "#".repeat((b * 44 / max) as usize));
    bins.iter().enumerate().map(row).collect()
}

fn join(xs: &[usize]) -> String {
    xs.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
}

fn human(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}k", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many values the totals keep in memory: what grows with a run's
    /// length, if anything.
    fn retained(c: &Counts) -> usize {
        c.wave_sizes.len() + c.durations.len() + c.scores.len() + c.sent.len()
    }

    // TRANSPORT.md, rule 3: the totals `--stats` prints don't grow with the
    // run; an endless stream would otherwise keep every score and duration.
    #[test]
    fn statistics_are_fixed_size() {
        let mut c = Counts::default();
        for n in 1..=100_000u64 {
            c.add(n, &Event::Wave { wave: n, questions: 24, requests: vec![n] });
            let size = Event::Request {
                request: n,
                sender: 0,
                questions: vec![n],
                bytes: 9000,
                body: None,
                pictures: Default::default(),
            };
            c.add(n, &size);
            let scores = vec![(n % 100) as f64 / 100.0; 24];
            let response = None;
            let ok = Event::Answered { request: n, result: "ok".into(), attempts: 1, tokens: 300, scores, response };
            c.add(n + 5, &ok);
        }
        assert!(retained(&c) <= 1_000, "{} values kept", retained(&c));
        let report = c.report(100_000 * 10, None);
        assert!(report.contains("waves 100000"), "{report}");
        assert!(report.contains("score histogram (2400000 float answers)"), "{report}");
    }

    fn line(c: &Counts) -> String {
        c.report(1_000_000, None).lines().nth(1).unwrap().to_owned()
    }

    /// Totals of requests that took `xs` milliseconds.
    fn took(xs: &[u64]) -> Counts {
        let mut c = Counts::default();
        for (n, &x) in (0..).zip(xs) {
            let sent = Event::Request {
                request: n,
                sender: 0,
                questions: Vec::new(),
                bytes: 0,
                body: None,
                pictures: Default::default(),
            };
            c.add(0, &sent);
            let answered = Event::Answered {
                request: n,
                result: "ok".into(),
                attempts: 1,
                tokens: 0,
                scores: Vec::new(),
                response: None,
            };
            c.add(x * 1000, &answered);
        }
        c
    }

    // Durations are kept to 2% above 100 ms: 300 ms reads as 298 ms.
    #[test]
    fn percentiles_by_nearest_rank() {
        let ms = took;
        let (none, one, two) = (ms(&[]), ms(&[300]), ms(&[800, 300]));
        assert!(line(&none).contains("p50 0ms · p99 0ms"), "{}", line(&none));
        assert!(line(&one).contains("p50 298ms · p99 298ms"), "{}", line(&one));
        assert!(line(&two).contains("p50 298ms · p99 800ms"), "the slower of two: {}", line(&two));
        assert_eq!(two.durations_under(500_000), 1);
        let hundred = ms(&(1..=100).collect::<Vec<_>>());
        assert!(line(&hundred).contains("p50 50ms · p99 99ms"), "{}", line(&hundred));
    }
}
