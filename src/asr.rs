//! iapp/OpenThai2.0-ASR-Streaming (ONNX): FastConformer encoder + greedy RNN-T decode.
//! The whole utterance is encoded at once, which suits push-to-talk dictation.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::value::{Tensor, TensorRef};

use crate::features::{N_MELS, SAMPLE_RATE, log_mel};

const PROMPT_THAI: i64 = 32;
const PROMPT_THAI_ENGLISH: i64 = 63;
const ENC_DIM: usize = 1024;
const STATE_LEN: usize = 2 * 640; // [2, 1, 640]
const MAX_SYMBOLS_PER_FRAME: usize = 10;

pub struct OpenThaiAsr {
    _env: ort::environment::Environment,
    encoder: Session,
    decoder: Session,
    tokens: Vec<String>,
    blank: usize,
}

impl OpenThaiAsr {
    pub fn load(dir: &Path) -> Result<Self> {
        let env = ort::init().with_name("poodpim").build()?;
        let env_n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let threads = env_n("POODPIM_ENC_THREADS", cores.min(4));
        let dec_threads = env_n("POODPIM_DEC_THREADS", 2);
        let build = |path: std::path::PathBuf, threads: usize| -> Result<Session> {
            Session::builder(&env)?
                // ort defaults to MaxEfficiency, which on macOS routes the model through CoreML;
                // for this model that is ~2x slower than plain CPU, so pin it to the CPU.
                .with_auto_device(AutoDevicePolicy::PreferCPU)
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::All))
                .and_then(|b| b.with_intra_threads(threads))
                .map_err(|e| anyhow!("{e}"))?
                .commit_from_file(&path)
                .with_context(|| format!("loading {}", path.display()))
        };
        let enc_path = crate::model::encoder_path(dir).ok_or_else(|| anyhow!("no encoder in {}", dir.display()))?;
        let encoder = build(enc_path, threads)?;
        let decoder = build(dir.join("decoder_joint.onnx"), dec_threads)?;
        let tokens: Vec<String> = std::fs::read_to_string(dir.join("tokens.txt"))
            .context("reading tokens.txt")?
            .lines()
            .map(str::to_owned)
            .collect();
        let blank = tokens.len();
        let mut asr = Self { _env: env, encoder, decoder, tokens, blank };
        // ONNX Runtime is slow on its first runs (allocations, kernel set-up); warm it up
        // so the first real dictation is fast.
        let noise: Vec<f32> = (0..SAMPLE_RATE as usize * 3).map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.01).collect();
        for _ in 0..2 {
            asr.transcribe(&noise, true)?;
        }
        Ok(asr)
    }

    pub fn transcribe(&mut self, audio: &[f32], mixed_english: bool) -> Result<String> {
        if audio.len() < (SAMPLE_RATE as usize) / 5 {
            return Ok(String::new());
        }
        let t_start = std::time::Instant::now();
        let (feats, t) = log_mel(audio);
        let t_mel = t_start.elapsed();
        let prompt = if mixed_english { PROMPT_THAI_ENGLISH } else { PROMPT_THAI };
        let enc_out = self.encoder.run(ort::inputs![
            "audio_signal" => Tensor::from_array(([1usize, N_MELS, t], feats))?,
            "length" => Tensor::from_array(([1usize], vec![t as i64]))?,
            "prompt_index" => Tensor::from_array(([1usize], vec![prompt]))?,
        ])?;
        let (shape, data) = enc_out["encoded"].try_extract_tensor::<f32>()?;
        let mut frames = shape[1] as usize;
        let (_, enc_len) = enc_out["encoded_length"].try_extract_tensor::<i64>()?;
        frames = frames.min(enc_len[0] as usize);
        let encoded: Vec<f32> = data[..frames * ENC_DIM].to_vec();
        drop(enc_out);
        let t_enc = t_start.elapsed();

        let blank_bias = if audio.len() < 2 * SAMPLE_RATE as usize { -2.5 } else { -1.0 };
        let ids = self.greedy(&encoded, frames, blank_bias)?;
        if std::env::var_os("POODPIM_TIMING").is_some() {
            eprintln!("mel {:?} enc {:?} dec {:?} frames {frames} tokens {}", t_mel, t_enc - t_mel, t_start.elapsed() - t_enc, ids.len());
        }
        Ok(self.detokenize(&ids))
    }

    fn greedy(&mut self, enc: &[f32], frames: usize, blank_bias: f32) -> Result<Vec<usize>> {
        let mut s1 = vec![0.0f32; STATE_LEN];
        let mut s2 = vec![0.0f32; STATE_LEN];
        let mut last = [self.blank as i32];
        let one = [1i32];
        let mut out = Vec::new();
        for t in 0..frames {
            let frame = &enc[t * ENC_DIM..(t + 1) * ENC_DIM]; // [1, 1024, 1]
            for _ in 0..MAX_SYMBOLS_PER_FRAME {
                let res = self.decoder.run(ort::inputs![
                    "encoder_outputs" => TensorRef::from_array_view(([1usize, ENC_DIM, 1], frame))?,
                    "targets" => TensorRef::from_array_view(([1usize, 1], &last[..]))?,
                    "target_length" => TensorRef::from_array_view(([1usize], &one[..]))?,
                    "input_states_1" => TensorRef::from_array_view(([2usize, 1, 640], &s1[..]))?,
                    "input_states_2" => TensorRef::from_array_view(([2usize, 1, 640], &s2[..]))?,
                ])?;
                let (_, scores) = res["outputs"].try_extract_tensor::<f32>()?;
                let mut best = 0usize;
                let mut best_v = f32::NEG_INFINITY;
                for (i, &v) in scores.iter().enumerate() {
                    let v = if i == self.blank { v + blank_bias } else { v };
                    if v > best_v {
                        best_v = v;
                        best = i;
                    }
                }
                if best == self.blank {
                    break;
                }
                out.push(best);
                last[0] = best as i32;
                s1.copy_from_slice(res["output_states_1"].try_extract_tensor::<f32>()?.1);
                s2.copy_from_slice(res["output_states_2"].try_extract_tensor::<f32>()?.1);
            }
        }
        Ok(out)
    }

    fn detokenize(&self, ids: &[usize]) -> String {
        let text: String = ids.iter().filter_map(|&i| self.tokens.get(i)).map(String::as_str).collect();
        // The vocabulary spells sara am as nikhahit + sara aa (U+0E4D U+0E32); use the
        // single code point U+0E33 (ำ) so search and spell-check work on the result.
        text.replace('\u{2581}', " ").replace("\u{0E4D}\u{0E32}", "\u{0E33}").trim().to_string()
    }
}
