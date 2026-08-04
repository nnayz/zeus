//! Pure status-chime synthesis plus an opt-in playback boundary.

use std::io;

pub const SAMPLE_RATE: u32 = 44_100;
pub const CHANNELS: u16 = 1;
pub const BITS_PER_SAMPLE: u16 = 16;
pub const PLAYBACK_VOLUME: f32 = 0.9;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StatusSound {
    NeedsInput,
    Done,
    Frozen,
}

#[derive(Clone, Copy)]
struct Strike {
    frequency: f64,
    start: f64,
    decay: f64,
    amplitude: f64,
}

const PARTIALS: [(f64, f64); 3] = [(1.0, 1.0), (2.0, 0.35), (3.0, 0.12)];

const NEEDS_INPUT: [Strike; 2] = [
    Strike {
        frequency: 587.33,
        start: 0.0,
        decay: 0.16,
        amplitude: 0.24,
    },
    Strike {
        frequency: 587.33,
        start: 0.14,
        decay: 0.26,
        amplitude: 0.24,
    },
];
const DONE: [Strike; 2] = [
    Strike {
        frequency: 659.25,
        start: 0.0,
        decay: 0.18,
        amplitude: 0.20,
    },
    Strike {
        frequency: 987.77,
        start: 0.11,
        decay: 0.34,
        amplitude: 0.17,
    },
];
const FROZEN: [Strike; 1] = [Strike {
    frequency: 220.0,
    start: 0.0,
    decay: 0.45,
    amplitude: 0.22,
}];

/// Render one 44.1 kHz, signed 16-bit, mono PCM RIFF/WAVE chime.
#[must_use]
pub fn synthesize_wav(event: StatusSound) -> Vec<u8> {
    let strikes: &[Strike] = match event {
        StatusSound::NeedsInput => &NEEDS_INPUT,
        StatusSound::Done => &DONE,
        StatusSound::Frozen => &FROZEN,
    };
    let tail = strikes
        .iter()
        .map(|strike| strike.start + strike.decay * 5.0)
        .fold(0.0_f64, f64::max);
    let frame_count = (tail * f64::from(SAMPLE_RATE)) as usize;
    let mut samples = Vec::with_capacity(frame_count);

    for index in 0..frame_count {
        let time = index as f64 / f64::from(SAMPLE_RATE);
        let mut value = 0.0;
        for strike in strikes.iter().filter(|strike| time >= strike.start) {
            let local = time - strike.start;
            let attack = (local / 0.003).min(1.0);
            let envelope = attack * (-local / strike.decay).exp() * strike.amplitude;
            for (harmonic, weight) in PARTIALS {
                value += envelope
                    * weight
                    * (2.0 * std::f64::consts::PI * strike.frequency * harmonic * local).sin();
            }
        }
        samples.push((value.clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16);
    }
    wav_container(&samples)
}

fn wav_container(samples: &[i16]) -> Vec<u8> {
    let data_size = u32::try_from(samples.len() * 2).expect("status sound fits in a WAV");
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Small playback seam: tests can inject a recorder and never open an audio device.
pub trait Player {
    fn play(&self, wav: &[u8], volume: f32) -> io::Result<()>;
}

pub fn play<P: Player>(player: &P, event: StatusSound) -> io::Result<()> {
    player.play(&synthesize_wav(event), PLAYBACK_VOLUME)
}

/// macOS's built-in player, available only when explicitly requested.
#[cfg(all(target_os = "macos", feature = "audio-playback"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct AfplayPlayer;

#[cfg(all(target_os = "macos", feature = "audio-playback"))]
impl Player for AfplayPlayer {
    fn play(&self, wav: &[u8], volume: f32) -> io::Result<()> {
        use std::{
            fs::{self, OpenOptions},
            io::Write,
            process::Command,
            sync::atomic::{AtomicU64, Ordering},
            thread,
        };

        static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "diri-status-sound-{}-{sequence}.wav",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(wav)?;
        drop(file);

        let child = Command::new("/usr/bin/afplay")
            .arg("--volume")
            .arg(volume.to_string())
            .arg(&path)
            .spawn();
        match child {
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                    let _ = fs::remove_file(path);
                });
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(path);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_headers_frame_counts_and_peaks_match_the_swift_synth() {
        for (event, expected_frames, expected_peak) in [
            (StatusSound::NeedsInput, 63_504, 10_578_i16),
            (StatusSound::Done, 79_821, 9_651_i16),
            (StatusSound::Frozen, 99_225, 8_427_i16),
        ] {
            let wav = synthesize_wav(event);
            assert_eq!(&wav[0..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            assert_eq!(&wav[36..40], b"data");
            assert_eq!(
                u32::from_le_bytes(wav[24..28].try_into().unwrap()),
                SAMPLE_RATE
            );
            assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
            assert_eq!((wav.len() - 44) / 2, expected_frames);
            let peak = wav[44..]
                .chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]).unsigned_abs())
                .max()
                .unwrap();
            assert!(peak.abs_diff(expected_peak.unsigned_abs()) <= 2);
        }
    }

    #[test]
    fn playback_uses_synthesized_bytes_and_swift_volume() {
        use std::cell::RefCell;

        #[derive(Default)]
        struct Recorder(RefCell<Option<(Vec<u8>, f32)>>);

        impl Player for Recorder {
            fn play(&self, wav: &[u8], volume: f32) -> io::Result<()> {
                self.0.replace(Some((wav.to_vec(), volume)));
                Ok(())
            }
        }

        let recorder = Recorder::default();
        play(&recorder, StatusSound::Done).unwrap();
        let recorded = recorder.0.borrow();
        let (wav, volume) = recorded.as_ref().unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(*volume, 0.9);
    }
}
