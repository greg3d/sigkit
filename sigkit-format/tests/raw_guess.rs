//! Seam: dtype guessing on generator output. Decoding a prefix of a raw file
//! as each plausible dtype and scoring plausibility picks the dtype (and byte
//! order) the generator wrote, for every supported dtype.

use sigkit_format::raw::guess;
use sigkit_format::synth::{self, RawSpec, Signal};
use sigkit_format::{ByteOrder, Dtype};

fn generated(dtype: Dtype, byte_order: ByteOrder, channels: usize, seed: u64) -> Vec<u8> {
    synth::raw(&RawSpec {
        dtype,
        byte_order,
        channels,
        seed,
        ..RawSpec::default()
    })
    .bytes
}

#[test]
fn the_guess_prefers_the_generators_dtype_on_its_own_output_for_each_supported_dtype() {
    for dtype in Dtype::ALL {
        for seed in 1..=5 {
            let g = guess(&generated(dtype, ByteOrder::Little, 1, seed));
            assert_eq!(g.dtype, dtype, "seed {seed}: guessed {g:?}");
            assert_eq!(
                g.byte_order,
                ByteOrder::Little,
                "{dtype} seed {seed}: guessed {g:?}"
            );
        }
    }
}

#[test]
fn the_guess_finds_a_big_endian_file_big_endian() {
    for dtype in Dtype::ALL {
        let g = guess(&generated(dtype, ByteOrder::Big, 1, 7));
        assert_eq!(
            (g.dtype, g.byte_order),
            (dtype, ByteOrder::Big),
            "guessed {g:?}"
        );
    }
}

#[test]
fn the_guess_holds_on_interleaved_channels_and_counts_them() {
    for dtype in Dtype::ALL {
        for channels in [2, 3] {
            let g = guess(&generated(dtype, ByteOrder::Little, channels, 3));
            assert_eq!(
                (g.dtype, g.channels),
                (dtype, channels),
                "{channels} channels: guessed {g:?}"
            );
        }
    }
}

#[test]
fn the_guess_holds_on_a_noisier_signal_and_a_single_channel_is_one_channel() {
    for dtype in Dtype::ALL {
        let bytes = synth::raw(&RawSpec {
            dtype,
            signal: Signal {
                freq_hz: 3.0,
                amplitude: 1.0,
                noise_sd: 0.2,
            },
            ..RawSpec::default()
        })
        .bytes;
        let g = guess(&bytes);
        assert_eq!((g.dtype, g.channels), (dtype, 1), "guessed {g:?}");
    }
}

#[test]
fn an_empty_or_tiny_prefix_guesses_without_panicking() {
    for bytes in [&[][..], &[1u8, 2, 3][..]] {
        let g = guess(bytes);
        assert!(g.channels >= 1);
    }
}
