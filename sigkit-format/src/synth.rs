//! Synthetic recordings with known truth, so every feature can be verified
//! before any phone logger exists.
//!
//! Model: the sensor samples a physical signal at exactly the nominal rate.
//! The device clock drifts, so stamps advance by `1 / (rate · (1 + ppm·1e-6))`
//! clock seconds per sample; a positive `ppm` is what the fitted rate reports
//! as ppm from nominal. Jitter is independent Gaussian noise on each stamp.
//! Dropped samples are removed at random, so their gaps are whole intervals.
//! The preset and the injected truth are written to the header under `source`.

use crate::column::{ByteOrder, Column, Dtype};
use crate::header::{Channel, ClockDomain, Header, Stream, UtcAnchor};
use crate::recording::{Recording, StreamData};
use crate::wallclock::rfc3339_now;
use serde_json::{Value, json};
use std::f64::consts::TAU;
use std::path::Path;

/// Clock domain of generated time columns.
pub const CLOCK_DOMAIN: &str = "sigkit.synthetic";
/// Stream id of the generated signal.
pub const STREAM_ID: &str = "signal";
/// Channel id of the generated signal.
pub const CHANNEL_ID: &str = "value";

/// Wall-clock instant of the physical zero of every generated recording:
/// 2026-10-29T12:00:00Z.
const UTC_ZERO_NS: i64 = 1_793_275_200_000_000_000;
/// Stamp of the recording's own zero on the clock of a single timestamped
/// recording, and of recording a in a pair: 1000 s of uptime.
const CLOCK_A_ZERO_NS: i64 = 1_000_000_000_000;
/// Stamp of recording b's zero in a pair: a different uptime, so each
/// recording visibly has its own zero.
const CLOCK_B_ZERO_NS: i64 = 5_000_000_000_000;

/// First count of a generated sample counter: not zero, so a reader that
/// forgets to count from the first stamp shows it.
pub const COUNTER_ZERO: i64 = 1_000_000;

/// A sine plus Gaussian noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signal {
    pub freq_hz: f64,
    pub amplitude: f64,
    pub noise_sd: f64,
}

impl Default for Signal {
    fn default() -> Self {
        Signal {
            freq_hz: 1.0,
            amplitude: 1.0,
            noise_sd: 0.01,
        }
    }
}

impl Signal {
    fn sample(&self, physical_s: f64, rng: &mut Rng) -> f32 {
        (self.amplitude * (TAU * self.freq_hz * physical_s).sin() + self.noise_sd * rng.gaussian())
            as f32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FixedRateSpec {
    pub name: String,
    pub rate_hz: f64,
    pub duration_s: f64,
    pub signal: Signal,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimestampedSpec {
    pub name: String,
    pub rate_nominal_hz: f64,
    pub duration_s: f64,
    /// Standard deviation of the per-stamp jitter, in seconds.
    pub jitter_s: f64,
    /// Probability that any sample after the first is dropped.
    pub drop_fraction: f64,
    /// Clock drift: the fitted rate deviates from nominal by this many ppm.
    pub ppm: f64,
    pub signal: Signal,
    pub seed: u64,
}

/// Two recordings (`<name>-a`, `<name>-b`) of one physical signal on two
/// drifting clocks. Recording b starts `shift_s` after a.
#[derive(Debug, Clone, PartialEq)]
pub struct PairSpec {
    pub name: String,
    pub rate_nominal_hz: f64,
    pub duration_s: f64,
    pub jitter_s: f64,
    pub drop_fraction: f64,
    pub ppm_a: f64,
    pub ppm_b: f64,
    pub shift_s: f64,
    pub signal: Signal,
    pub seed: u64,
}

/// A legacy raw file with no header: `channels` interleaved channels of one
/// dtype in one byte order, after `header_bytes` of text the reader must skip.
/// Channel `j` is the signal at `j + 1` times its frequency, so channels
/// differ. Integer dtypes hold counts: the value times
/// [`counts_per_unit`], rounded and saturated.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSpec {
    pub dtype: Dtype,
    pub byte_order: ByteOrder,
    pub channels: usize,
    pub header_bytes: usize,
    pub rate_hz: f64,
    pub duration_s: f64,
    pub signal: Signal,
    pub seed: u64,
}

impl Default for RawSpec {
    fn default() -> Self {
        RawSpec {
            dtype: Dtype::F32,
            byte_order: ByteOrder::Little,
            channels: 1,
            header_bytes: 0,
            rate_hz: 100.0,
            duration_s: 10.0,
            signal: Signal::default(),
            seed: 1,
        }
    }
}

/// A generated raw file: its bytes, and each channel's stored values (counts
/// for an integer dtype), as a reader should decode them.
#[derive(Debug, Clone, PartialEq)]
pub struct RawFile {
    pub bytes: Vec<u8>,
    pub channels: Vec<Vec<f64>>,
}

/// Counts per unit of signal an integer raw file stores: an amplitude of one
/// spans a sixteenth of an `i16`, a millionth-ish step for wider ones.
pub fn counts_per_unit(dtype: Dtype) -> f64 {
    match dtype {
        Dtype::I16 => 2048.0,
        Dtype::I32 => 1048576.0,
        Dtype::I64 => 4294967296.0,
        Dtype::F32 | Dtype::F64 => 1.0,
    }
}

/// A legacy raw file with no header (see [`RawSpec`]).
pub fn raw(spec: &RawSpec) -> RawFile {
    const TEXT: &[u8] = b"legacy acquisition header, not samples\n";
    let mut rng = Rng::new(spec.seed);
    let n = (spec.duration_s * spec.rate_hz).round() as usize;
    let scale = counts_per_unit(spec.dtype);
    let mut bytes: Vec<u8> = TEXT
        .iter()
        .copied()
        .cycle()
        .take(spec.header_bytes)
        .collect();
    let mut channels = vec![Vec::with_capacity(n); spec.channels];
    for i in 0..n {
        let t = i as f64 / spec.rate_hz;
        for (j, values) in channels.iter_mut().enumerate() {
            let signal = Signal {
                freq_hz: spec.signal.freq_hz * (j + 1) as f64,
                ..spec.signal
            };
            let encoded = spec
                .dtype
                .encode(signal.sample(t, &mut rng) as f64 * scale, spec.byte_order);
            values.push(spec.dtype.decode(&encoded, spec.byte_order));
            bytes.extend_from_slice(&encoded);
        }
    }
    RawFile { bytes, channels }
}

/// A generated recording, in memory: its header and its streams' samples.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticRecording {
    pub header: Header,
    pub streams: Vec<StreamData>,
}

impl SyntheticRecording {
    /// The same recording stamped on `clock` instead of the synthetic
    /// nanosecond clock, for checking clock-domain handling:
    /// - a unit of time (`ios.systemUptime` in `us`, say): stamps and UTC
    ///   anchors are converted to it, rounding down, so the recording reads
    ///   as the same times in seconds;
    /// - a sample counter: stamps become a hardware counter from
    ///   [`COUNTER_ZERO`], stepping by the nominal intervals between
    ///   consecutive stamps (one, plus one per dropped sample), as a counter
    ///   clocked with the samples would. It neither jitters nor drifts
    ///   against itself. The anchor moves to the first count.
    pub fn on_clock(mut self, clock: ClockDomain) -> SyntheticRecording {
        for (stream, data) in self.header.streams.iter_mut().zip(&mut self.streams) {
            let nominal_hz = stream.rate_nominal_hz.unwrap_or(1.0);
            if let Some(time) = &mut data.time {
                if clock.is_sample_counter() {
                    let mut count = COUNTER_ZERO;
                    let mut previous = time.first().copied().unwrap_or(0);
                    for stamp in time.iter_mut() {
                        let intervals = ((*stamp - previous) as f64 * nominal_hz / 1e9).round();
                        count += (intervals as i64).max((*stamp != previous) as i64);
                        previous = *stamp;
                        *stamp = count;
                    }
                } else {
                    let per_tick = clock.nanos_per_tick().expect("a unit this crate knows");
                    for stamp in time.iter_mut() {
                        *stamp = stamp.div_euclid(per_tick);
                    }
                }
            }
            if let Some(anchor) = &mut stream.utc_anchor {
                anchor.mono = match clock.nanos_per_tick() {
                    Some(per_tick) => anchor.mono.div_euclid(per_tick),
                    None => COUNTER_ZERO,
                };
            }
            stream.time = Some(clock.clone());
        }
        self
    }

    /// Write to `dir` as a `.sigrec` recording.
    pub fn write(&self, dir: impl AsRef<Path>) -> crate::Result<Recording> {
        Recording::create(dir, self.header.clone(), &self.streams)
    }
}

/// A fixed-rate sine plus noise.
pub fn fixed_rate(spec: &FixedRateSpec) -> SyntheticRecording {
    let mut rng = Rng::new(spec.seed);
    let n = (spec.duration_s * spec.rate_hz).round() as usize;
    let values: Vec<f32> = (0..n)
        .map(|i| spec.signal.sample(i as f64 / spec.rate_hz, &mut rng))
        .collect();

    let mut stream = Stream::fixed_rate(STREAM_ID, spec.rate_hz);
    stream
        .channels
        .push(Channel::new(CHANNEL_ID, Dtype::F32, ""));
    SyntheticRecording {
        header: header(
            &spec.name,
            stream,
            "fixed_rate",
            &spec.signal,
            json!({ "seed": spec.seed }),
        ),
        streams: vec![StreamData::fixed_rate(vec![Column::F32(values)])],
    }
}

/// A timestamped sine plus noise with jitter, dropped samples and clock drift.
pub fn timestamped(spec: &TimestampedSpec) -> SyntheticRecording {
    let zero = ClockZero {
        stamp_ns: CLOCK_A_ZERO_NS,
        physical_s: 0.0,
    };
    let truth = json!({
        "ppm": spec.ppm,
        "jitter_s": spec.jitter_s,
        "drop_fraction": spec.drop_fraction,
        "seed": spec.seed,
    });
    timestamped_on_clock(spec, zero, "timestamped", truth)
}

/// Two recordings of the same physical signal on two drifting clocks, the
/// second shifted by `shift_s`.
pub fn pair(spec: &PairSpec) -> [SyntheticRecording; 2] {
    let take = |suffix: &str, ppm: f64, stamp_ns: i64, physical_s: f64, seed: u64| {
        let one = TimestampedSpec {
            name: format!("{}-{suffix}", spec.name),
            rate_nominal_hz: spec.rate_nominal_hz,
            duration_s: spec.duration_s,
            jitter_s: spec.jitter_s,
            drop_fraction: spec.drop_fraction,
            ppm,
            signal: spec.signal,
            seed,
        };
        let truth = json!({
            "pair": suffix,
            "ppm": ppm,
            "shift_s": physical_s,
            "jitter_s": spec.jitter_s,
            "drop_fraction": spec.drop_fraction,
            "seed": seed,
        });
        timestamped_on_clock(
            &one,
            ClockZero {
                stamp_ns,
                physical_s,
            },
            "pair",
            truth,
        )
    };
    [
        take("a", spec.ppm_a, CLOCK_A_ZERO_NS, 0.0, spec.seed),
        take(
            "b",
            spec.ppm_b,
            CLOCK_B_ZERO_NS,
            spec.shift_s,
            spec.seed.wrapping_add(1),
        ),
    ]
}

/// Where a recording's own zero sits: its stamp on the device clock, and the
/// physical time it corresponds to.
struct ClockZero {
    stamp_ns: i64,
    physical_s: f64,
}

fn timestamped_on_clock(
    spec: &TimestampedSpec,
    zero: ClockZero,
    preset: &str,
    truth: Value,
) -> SyntheticRecording {
    let mut rng = Rng::new(spec.seed);
    let n = (spec.duration_s * spec.rate_nominal_hz).round() as usize;
    let clock_scale = 1.0 / (1.0 + spec.ppm * 1e-6);
    let mut stamps = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for i in 0..n {
        let since_start_s = i as f64 / spec.rate_nominal_hz;
        let value = spec
            .signal
            .sample(zero.physical_s + since_start_s, &mut rng);
        let jitter = spec.jitter_s * rng.gaussian();
        let dropped = i > 0 && rng.uniform() < spec.drop_fraction;
        if dropped {
            continue;
        }
        let clock_s = since_start_s * clock_scale + jitter;
        stamps.push(zero.stamp_ns + (clock_s * 1e9).round() as i64);
        values.push(value);
    }

    let mut stream = Stream::timestamped(
        STREAM_ID,
        ClockDomain::new(CLOCK_DOMAIN, "ns"),
        spec.rate_nominal_hz,
    );
    stream.utc_anchor = Some(UtcAnchor::new(
        zero.stamp_ns,
        UTC_ZERO_NS + (zero.physical_s * 1e9).round() as i64,
    ));
    stream
        .channels
        .push(Channel::new(CHANNEL_ID, Dtype::F32, ""));
    SyntheticRecording {
        header: header(&spec.name, stream, preset, &spec.signal, truth),
        streams: vec![StreamData::timestamped(stamps, vec![Column::F32(values)])],
    }
}

/// A one-stream header whose `source` records the preset, the signal and the
/// injected truth.
fn header(name: &str, stream: Stream, preset: &str, signal: &Signal, truth: Value) -> Header {
    let mut header = Header::new(name);
    header.created = Some(rfc3339_now());
    header.source = Some(json!({
        "app": format!("sigkit gen {}", env!("CARGO_PKG_VERSION")),
        "preset": preset,
        "signal": { "freq_hz": signal.freq_hz, "amplitude": signal.amplitude, "noise_sd": signal.noise_sd },
        "truth": truth,
    }));
    header.streams.push(stream);
    header
}

/// SplitMix64: small, seedable, good enough for noise.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal, by Box–Muller.
    fn gaussian(&mut self) -> f64 {
        let u1 = 1.0 - self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}
