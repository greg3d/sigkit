//! `sigkit`: generate synthetic recordings with known truth, and inspect recordings.

use clap::{Args, Parser, Subcommand};
use sigkit_format::synth::{self, FixedRateSpec, PairSpec, Signal, TimestampedSpec};
use sigkit_format::wallclock::rfc3339_utc;
use sigkit_format::{Recording, Stream, StreamKind, fit_rate, ppm_from_nominal};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "sigkit",
    version,
    about = "Generate and inspect .sigrec recordings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a synthetic recording with known truth
    Gen {
        #[command(subcommand)]
        preset: Preset,
    },
    /// Print a recording's streams, channels, counts, duration and rates
    Inspect {
        /// A .sigrec directory
        recording: PathBuf,
    },
}

#[derive(Subcommand)]
enum Preset {
    /// A fixed-rate sine plus noise
    FixedRate {
        /// Output recording (`.sigrec` is appended when missing)
        out: PathBuf,
        /// Sample rate in Hz
        #[arg(long, default_value_t = 100.0)]
        rate: f64,
        /// Length in seconds
        #[arg(long, default_value_t = 10.0)]
        duration: f64,
        #[command(flatten)]
        signal: SignalArgs,
    },
    /// A timestamped sine with jitter, dropped samples and clock drift
    Timestamped {
        /// Output recording (`.sigrec` is appended when missing)
        out: PathBuf,
        #[command(flatten)]
        clock: ClockArgs,
        /// Clock drift: the fitted rate deviates from nominal by this many ppm
        #[arg(long, default_value_t = 100.0, allow_negative_numbers = true)]
        ppm: f64,
        #[command(flatten)]
        signal: SignalArgs,
    },
    /// Two recordings of one signal on two drifting clocks, the second shifted
    Pair {
        /// Output base: writes `<out>-a.sigrec` and `<out>-b.sigrec`
        out: PathBuf,
        #[command(flatten)]
        clock: ClockArgs,
        /// Clock drift of recording a, in ppm
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        ppm_a: f64,
        /// Clock drift of recording b, in ppm
        #[arg(long, default_value_t = 150.0, allow_negative_numbers = true)]
        ppm_b: f64,
        /// Recording b starts this many seconds after a
        #[arg(long, default_value_t = 0.25, allow_negative_numbers = true)]
        shift: f64,
        #[command(flatten)]
        signal: SignalArgs,
    },
}

#[derive(Args)]
struct ClockArgs {
    /// Nominal sample rate in Hz
    #[arg(long, default_value_t = 416.0)]
    rate: f64,
    /// Length in seconds
    #[arg(long, default_value_t = 10.0)]
    duration: f64,
    /// Stamp jitter, standard deviation in microseconds
    #[arg(long, default_value_t = 50.0)]
    jitter_us: f64,
    /// Probability that any sample after the first is dropped
    #[arg(long, default_value_t = 0.0)]
    drop: f64,
}

#[derive(Args)]
struct SignalArgs {
    /// Sine frequency in Hz
    #[arg(long, default_value_t = 1.0)]
    freq: f64,
    /// Sine amplitude
    #[arg(long, default_value_t = 1.0)]
    amplitude: f64,
    /// Gaussian noise, standard deviation
    #[arg(long, default_value_t = 0.01)]
    noise: f64,
    /// Random seed
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

impl SignalArgs {
    fn signal(&self) -> Signal {
        Signal {
            freq_hz: self.freq,
            amplitude: self.amplitude,
            noise_sd: self.noise,
        }
    }
}

fn main() {
    if let Err(err) = run(Cli::parse()) {
        eprintln!("sigkit: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Gen { preset } => generate(preset),
        Command::Inspect { recording } => inspect(&recording),
    }
}

fn generate(preset: Preset) -> Result<(), Box<dyn std::error::Error>> {
    match preset {
        Preset::FixedRate {
            out,
            rate,
            duration,
            signal,
        } => {
            let dir = recording_dir(&out);
            let synthetic = synth::fixed_rate(&FixedRateSpec {
                name: recording_name(&dir),
                rate_hz: rate,
                duration_s: duration,
                signal: signal.signal(),
                seed: signal.seed,
            });
            let rec = synthetic.write(&dir)?;
            println!(
                "wrote {} ({} samples at {rate} Hz)",
                dir.display(),
                rec.sample_count(synth::STREAM_ID).unwrap_or(0)
            );
        }
        Preset::Timestamped {
            out,
            clock,
            ppm,
            signal,
        } => {
            let dir = recording_dir(&out);
            let synthetic = synth::timestamped(&TimestampedSpec {
                name: recording_name(&dir),
                rate_nominal_hz: clock.rate,
                duration_s: clock.duration,
                jitter_s: clock.jitter_us / 1e6,
                drop_fraction: clock.drop,
                ppm,
                signal: signal.signal(),
                seed: signal.seed,
            });
            let rec = synthetic.write(&dir)?;
            println!(
                "wrote {} ({} samples, nominal {} Hz, {ppm:+} ppm, jitter {} us, drop {})",
                dir.display(),
                rec.sample_count(synth::STREAM_ID).unwrap_or(0),
                clock.rate,
                clock.jitter_us,
                clock.drop
            );
        }
        Preset::Pair {
            out,
            clock,
            ppm_a,
            ppm_b,
            shift,
            signal,
        } => {
            let base = pair_base(&out);
            let [a, b] = synth::pair(&PairSpec {
                name: recording_name(&base),
                rate_nominal_hz: clock.rate,
                duration_s: clock.duration,
                jitter_s: clock.jitter_us / 1e6,
                drop_fraction: clock.drop,
                ppm_a,
                ppm_b,
                shift_s: shift,
                signal: signal.signal(),
                seed: signal.seed,
            });
            for (synthetic, ppm) in [(&a, ppm_a), (&b, ppm_b)] {
                let dir = base.with_file_name(format!("{}.sigrec", synthetic.header.name));
                let rec = synthetic.write(&dir)?;
                println!(
                    "wrote {} ({} samples, nominal {} Hz, {ppm:+} ppm)",
                    dir.display(),
                    rec.sample_count(synth::STREAM_ID).unwrap_or(0),
                    clock.rate
                );
            }
            println!("recording b starts {shift} s after a");
        }
    }
    Ok(())
}

/// `out` with `.sigrec` appended unless it already ends in it.
fn recording_dir(out: &Path) -> PathBuf {
    if out.extension().is_some_and(|e| e == "sigrec") {
        out.to_path_buf()
    } else {
        let mut name = out
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".sigrec");
        out.with_file_name(name)
    }
}

/// `out` with a trailing `.sigrec` removed: the base of `<base>-a.sigrec`.
fn pair_base(out: &Path) -> PathBuf {
    if out.extension().is_some_and(|e| e == "sigrec") {
        out.with_extension("")
    } else {
        out.to_path_buf()
    }
}

/// The recording's name: the directory name without `.sigrec`.
fn recording_name(dir: &Path) -> String {
    dir.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into())
}

fn inspect(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rec = Recording::open(path)?;
    let header = rec.header();
    let created = header
        .created
        .as_deref()
        .map(|c| format!(", created {c}"))
        .unwrap_or_default();
    println!(
        "{} ({}): {} stream{}{created}",
        header.name,
        path.display(),
        header.streams.len(),
        if header.streams.len() == 1 { "" } else { "s" }
    );
    for stream in &header.streams {
        let n = rec.sample_count(&stream.id).unwrap_or(0);
        match stream.kind {
            StreamKind::Timestamped => inspect_timestamped(&rec, stream, n)?,
            StreamKind::FixedRate => {
                let rate = stream.rate_hz.unwrap_or(f64::NAN);
                println!(
                    "  stream {}: fixed_rate, {n} samples over {:.3} s",
                    stream.id,
                    n as f64 / rate
                );
                println!("    rate {rate:.3} Hz");
            }
        }
        let tc = &stream.time_correction;
        println!("    time correction k {}, shift {} s", tc.k, tc.shift_s);
        if let Some(anchor) = &stream.utc_anchor {
            println!(
                "    utc anchor mono {} = {}",
                anchor.mono,
                rfc3339_utc(anchor.utc_ns.div_euclid(1_000_000_000))
            );
        }
        for ch in &stream.channels {
            println!(
                "    channel {}: {}, unit {:?}, gain {}, offset {}",
                ch.id, ch.dtype, ch.unit, ch.gain, ch.offset
            );
        }
    }
    Ok(())
}

fn inspect_timestamped(
    rec: &Recording,
    stream: &Stream,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let clock = stream.time.as_ref().expect("validated at open");
    let nominal = stream.rate_nominal_hz.expect("validated at open");
    let stamps = rec.read_time(&stream.id)?.unwrap_or_default();
    let Some(seconds_per_tick) = stream.seconds_per_tick() else {
        println!(
            "  stream {}: timestamped, {n} samples (unknown clock unit {:?})",
            stream.id, clock.unit
        );
        println!("    clock {} [{}]", clock.domain, clock.unit);
        println!("    rate nominal {nominal:.3} Hz, fitted: unknown clock unit");
        return Ok(());
    };
    let duration = match (stamps.first(), stamps.last()) {
        (Some(first), Some(last)) => (last - first) as f64 * seconds_per_tick,
        _ => 0.0,
    };
    let fit = fit_rate(&stamps, seconds_per_tick, nominal);
    let dropped = match fit {
        Some(fit) if fit.missing > 0 => format!(", {} dropped", fit.missing),
        _ => String::new(),
    };
    println!(
        "  stream {}: timestamped, {n} samples over {duration:.3} s{dropped}",
        stream.id
    );
    println!("    clock {} [{}]", clock.domain, clock.unit);
    match fit {
        Some(fit) => println!(
            "    rate nominal {nominal:.3} Hz, fitted {:.4} Hz ({:+.1} ppm)",
            fit.rate_hz,
            ppm_from_nominal(fit.rate_hz, nominal)
        ),
        None => println!("    rate nominal {nominal:.3} Hz, fitted: too few samples"),
    }
    Ok(())
}
