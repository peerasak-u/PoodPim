//! Microphone capture (any device rate, mixed to mono), resampling to 16 kHz,
//! silence trimming, and WAV loading for testing.

use std::io::{Cursor, Read, Seek};
use std::path::Path;

use anyhow::{Context, Result};

use crate::features::SAMPLE_RATE;

#[cfg(feature = "desktop")]
pub use recorder::{Recorder, default_input_name, input_names};

/// Reads the stored microphone name leniently: anything but a string (a hand-edited
/// `config.json`) means the system default instead of failing the whole file.
pub fn de_device_name<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
    Ok(v.as_str().map(str::to_string))
}

/// Reads the microphone list leniently: names are kept, null is the system default, anything
/// else (a hand-edited `config.json`) is dropped rather than failing the whole file.
pub fn de_mic_list<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Option<String>>, D::Error> {
    let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
    let items = v.as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|x| match x {
            serde_json::Value::Null => Some(None),
            serde_json::Value::String(s) => Some(Some(s.clone())),
            _ => None,
        })
        .collect())
}

/// Which of the connected `names` to open for the preference list `order`: the first entry that
/// is connected, matched by exact name. None for the system default: reached in the list, or
/// nothing listed is connected.
pub fn choose(names: &[String], order: &[Option<String>]) -> Option<usize> {
    for wanted in order {
        match wanted {
            None => return None,
            Some(w) => {
                if let Some(i) = names.iter().position(|n| n == w) {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Keeps a preference list sound: no blank or repeated names, and the system default exactly
/// once (at the end if it was missing).
pub fn tidy_mics(order: &mut Vec<Option<String>>) {
    let mut seen = std::collections::HashSet::new();
    let mut default = false;
    order.retain(|m| match m {
        None => !std::mem::replace(&mut default, true),
        Some(name) => !name.trim().is_empty() && seen.insert(name.clone()),
    });
    if !default {
        order.push(None);
    }
}

/// Adds connected microphones not in `order` yet to its end, so they can be moved up later.
/// Returns whether anything was added.
pub fn remember_mics(order: &mut Vec<Option<String>>, names: &[String]) -> bool {
    let before = order.len();
    for name in names {
        if !order.iter().any(|m| m.as_deref() == Some(name)) {
            order.push(Some(name.clone()));
        }
    }
    tidy_mics(order);
    order.len() != before
}

#[cfg(feature = "desktop")]
mod recorder {
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};

use super::resample;
use crate::features::SAMPLE_RATE;

pub struct Recorder {
    stream: Option<cpal::Stream>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    device_name: String,
}

impl Recorder {
    pub fn new() -> Self {
        Self { stream: None, buf: Arc::new(Mutex::new(Vec::new())), rate: SAMPLE_RATE, device_name: String::new() }
    }

    /// Starts recording from the first connected microphone in `order` (see `choose`).
    pub fn start(&mut self, order: &[Option<String>]) -> Result<()> {
        let device = pick(order)?;
        self.device_name = name_of(&device);
        let supported = device.default_input_config().context("microphone config")?;
        let config = supported.config();
        self.rate = config.sample_rate;
        self.buf.lock().unwrap().clear();
        let ch = config.channels as usize;
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, ch, self.buf.clone())?,
            cpal::SampleFormat::I16 => build::<i16>(&device, config, ch, self.buf.clone())?,
            cpal::SampleFormat::I32 => build::<i32>(&device, config, ch, self.buf.clone())?,
            cpal::SampleFormat::U16 => build::<u16>(&device, config, ch, self.buf.clone())?,
            f => return Err(anyhow!("unsupported microphone sample format {f:?}")),
        };
        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Opens the microphone (`order`, as in `start`) briefly on a background thread and closes
    /// it again. The first time a process opens it can take seconds (2.7 s measured on a Mac),
    /// long enough to lose the first words or a whole short phrase; afterwards it takes tens of ms.
    pub fn warm_up(order: Vec<Option<String>>) {
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let r = (|| -> Result<()> {
                let device = pick(&order)?;
                let supported = device.default_input_config().context("microphone config")?;
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::F32 => build::<f32>(&device, supported.config(), 1, Arc::new(Mutex::new(Vec::new())))?,
                    cpal::SampleFormat::I16 => build::<i16>(&device, supported.config(), 1, Arc::new(Mutex::new(Vec::new())))?,
                    cpal::SampleFormat::I32 => build::<i32>(&device, supported.config(), 1, Arc::new(Mutex::new(Vec::new())))?,
                    cpal::SampleFormat::U16 => build::<u16>(&device, supported.config(), 1, Arc::new(Mutex::new(Vec::new())))?,
                    f => return Err(anyhow!("unsupported microphone sample format {f:?}")),
                };
                stream.play()?;
                std::thread::sleep(std::time::Duration::from_millis(200));
                Ok(())
            })();
            match r {
                Ok(()) => eprintln!("microphone warmed up in {} ms", t0.elapsed().as_millis()),
                Err(e) => eprintln!("microphone warm-up: {e:#}"),
            }
        });
    }

    /// The name of the microphone the last `start` opened.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The microphone's sample rate (the buffer is mono at this rate).
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Runs `f` on the recording so far (mono, at `rate()`), without stopping it.
    pub fn with_buffer<R>(&self, f: impl FnOnce(&[f32]) -> R) -> R {
        f(&self.buf.lock().unwrap())
    }

    /// Stops recording and returns the whole recording (mono, at `rate()`).
    pub fn stop_raw(&mut self) -> Vec<f32> {
        self.stream = None; // dropping the stream stops it
        std::mem::take(&mut *self.buf.lock().unwrap())
    }

    /// Stops recording and returns 16 kHz mono audio.
    pub fn stop(&mut self) -> Vec<f32> {
        let raw = self.stop_raw();
        resample(&raw, self.rate, SAMPLE_RATE)
    }
}

/// The names of the connected input devices, in the system's order.
pub fn input_names() -> Vec<String> {
    match cpal::default_host().input_devices() {
        Ok(devices) => devices.map(|d| name_of(&d)).collect(),
        Err(e) => {
            eprintln!("listing microphones: {e}");
            Vec::new()
        }
    }
}

/// The name of the system's default input device, if there is one.
pub fn default_input_name() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| name_of(&d))
}

fn name_of(device: &cpal::Device) -> String {
    device.description().map(|d| d.name().to_string()).unwrap_or_default()
}

/// The first connected microphone in `order`, or the system default (see `choose`).
fn pick(order: &[Option<String>]) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if order.first().is_some_and(Option::is_some) {
        let devices: Vec<cpal::Device> = host.input_devices().map(|d| d.collect()).unwrap_or_default();
        let names: Vec<String> = devices.iter().map(name_of).collect();
        if let Some(i) = super::choose(&names, order) {
            return Ok(devices.into_iter().nth(i).expect("index from names"));
        }
    }
    host.default_input_device().ok_or_else(|| anyhow!("no microphone found"))
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, ch: usize, buf: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            let mut b = buf.lock().unwrap();
            for frame in data.chunks(ch) {
                let s: f32 = frame.iter().map(|&v| f32::from_sample(v)).sum::<f32>() / ch as f32;
                b.push(s);
            }
        },
        |e| eprintln!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}
}

/// Windowed-sinc resampler (Hann window, 16 zero-crossings).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let cutoff = ratio.min(1.0) * 0.95;
    let half = (16.0 / cutoff).ceil() as isize;
    let taps = 2 * half as usize;
    let weight = |d: f64| {
        let x = cutoff * d;
        let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
        let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half as f64).cos();
        cutoff * sinc * w
    };
    // Output sample n sits at input position n·from/to: a whole part and a fraction phase/period.
    // The fraction repeats every `period` samples, so each phase's tap weights are worked out
    // once. (A sin and a cos per tap per sample took 0.5 s for a 25 s part at 48 kHz, on the
    // event loop.) An odd rate with too many phases for a table computes them as it goes.
    let g = gcd(from, to) as u64;
    let (step, period) = (from as u64 / g, to as u64 / g);
    let weights_at = |phase: u64| {
        let frac = phase as f64 / period as f64;
        (0..taps).map(move |j| weight(frac + (half - 1 - j as isize) as f64))
    };
    let table: Option<Vec<f64>> = (period as usize * taps <= 1 << 20).then(|| (0..period).flat_map(weights_at).collect());
    let mut row = vec![0.0f64; taps];
    let out_len = (input.len() as f64 * ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len as u64 {
        let (center, phase) = ((n * step / period) as isize, n * step % period);
        let weights = match &table {
            Some(t) => &t[phase as usize * taps..][..taps],
            None => {
                row.iter_mut().zip(weights_at(phase)).for_each(|(r, w)| *r = w);
                &row[..]
            }
        };
        let first = center - half + 1;
        let mut acc = 0.0f64;
        for (j, w) in weights.iter().enumerate() {
            let k = first + j as isize;
            if k >= 0 && (k as usize) < input.len() {
                acc += input[k as usize] as f64 * w;
            }
        }
        out.push(acc as f32);
    }
    out
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Cuts leading/trailing silence (20 ms frames below -45 dBFS), keeping 250 ms padding.
pub fn trim_silence(audio: &[f32]) -> &[f32] {
    let frame = (SAMPLE_RATE / 50) as usize;
    let n = audio.len() / frame;
    if n == 0 {
        return audio;
    }
    let loud: Vec<usize> = (0..n)
        .filter(|&i| {
            let s = &audio[i * frame..(i + 1) * frame];
            let rms = (s.iter().map(|v| v * v).sum::<f32>() / frame as f32 + 1e-12).sqrt();
            20.0 * rms.log10() > -45.0
        })
        .collect();
    let (Some(&first), Some(&last)) = (loud.first(), loud.last()) else {
        return &audio[..0];
    };
    let pad = (SAMPLE_RATE as usize) / 4;
    let start = (first * frame).saturating_sub(pad);
    let end = ((last + 1) * frame + pad).min(audio.len());
    &audio[start..end]
}

/// Loads a WAV file as 16 kHz mono f32.
pub fn load_wav(path: &Path) -> Result<Vec<f32>> {
    let f = std::io::BufReader::new(std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?);
    read_wav(f)
}

/// Decodes WAV bytes (any rate / channels / PCM or float) to 16 kHz mono f32.
pub fn decode_wav(bytes: &[u8]) -> Result<Vec<f32>> {
    read_wav(Cursor::new(bytes))
}

fn read_wav<R: Read + Seek>(reader: R) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::new(reader).context("not a WAV file")?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok(resample(&mono, spec.sample_rate, SAMPLE_RATE))
}

/// Encodes 16 kHz mono audio as 16-bit PCM WAV (32 kB per second) for upload.
pub fn encode_wav(audio: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec { channels: 1, sample_rate: SAMPLE_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut buf = Cursor::new(Vec::with_capacity(44 + audio.len() * 2));
    {
        let mut w = hound::WavWriter::new(&mut buf, spec).expect("wav writer");
        for &s in audio {
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).expect("wav write");
        }
        w.finalize().expect("wav finalize");
    }
    buf.into_inner()
}

#[cfg(test)]
mod tests {
    use super::{choose, remember_mics, tidy_mics};

    fn names() -> Vec<String> {
        vec!["MacBook Pro Microphone".into(), "USB Audio Device".into(), "USB Audio Device".into()]
    }

    fn order(items: &[Option<&str>]) -> Vec<Option<String>> {
        items.iter().map(|m| m.map(str::to_string)).collect()
    }

    #[test]
    fn choose_takes_the_first_connected_in_order() {
        let o = order(&[Some("AirPods"), Some("USB Audio Device"), None, Some("MacBook Pro Microphone")]);
        assert_eq!(choose(&names(), &o), Some(1));
    }

    #[test]
    fn choose_stops_at_the_system_default() {
        let o = order(&[Some("AirPods"), None, Some("MacBook Pro Microphone")]);
        assert_eq!(choose(&names(), &o), None);
    }

    #[test]
    fn choose_matches_names_exactly() {
        assert_eq!(choose(&names(), &order(&[Some("usb audio device"), None])), None);
    }

    #[test]
    fn choose_with_nothing_connected_or_listed_uses_default() {
        assert_eq!(choose(&[], &order(&[Some("USB Audio Device"), None])), None);
        assert_eq!(choose(&names(), &[]), None);
    }

    #[test]
    fn tidy_keeps_one_default_and_no_repeats() {
        let mut o = order(&[Some("A"), None, Some(" "), Some("A"), None, Some("B")]);
        tidy_mics(&mut o);
        assert_eq!(o, order(&[Some("A"), None, Some("B")]));
        let mut o = order(&[Some("A")]);
        tidy_mics(&mut o);
        assert_eq!(o, order(&[Some("A"), None]));
    }

    #[test]
    fn remember_adds_new_microphones_at_the_end() {
        let mut o = order(&[Some("USB Audio Device"), None]);
        assert!(remember_mics(&mut o, &names()));
        assert_eq!(o, order(&[Some("USB Audio Device"), None, Some("MacBook Pro Microphone")]));
        assert!(!remember_mics(&mut o, &names()));
    }

    /// The resampler as first written: every tap's weight computed for every sample.
    fn resample_reference(input: &[f32], from: u32, to: u32) -> Vec<f32> {
        let ratio = to as f64 / from as f64;
        let cutoff = ratio.min(1.0) * 0.95;
        let half = (16.0 / cutoff).ceil() as isize;
        let out_len = (input.len() as f64 * ratio).floor() as usize;
        (0..out_len)
            .map(|n| {
                let t = n as f64 / ratio;
                let center = t.floor() as isize;
                let mut acc = 0.0f64;
                for k in (center - half + 1)..=(center + half) {
                    if k < 0 || k as usize >= input.len() {
                        continue;
                    }
                    let d = t - k as f64;
                    let x = cutoff * d;
                    let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                    let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half as f64).cos();
                    acc += input[k as usize] as f64 * cutoff * sinc * w;
                }
                acc as f32
            })
            .collect()
    }

    #[test]
    fn resample_matches_the_reference_at_common_and_odd_rates() {
        let input: Vec<f32> = (0..9_000).map(|i| ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.5).collect();
        for from in [48_000, 44_100, 22_050, 8_000, 96_000, 47_999] {
            let (fast, slow) = (super::resample(&input, from, 16_000), resample_reference(&input, from, 16_000));
            assert_eq!(fast.len(), slow.len(), "{from} Hz");
            let worst = fast.iter().zip(&slow).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "{from} Hz: off by {worst}");
        }
        assert_eq!(super::resample(&input, 16_000, 16_000), input);
    }

}
