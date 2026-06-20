use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use app_lib::nemotron_engine::model::NemotronModel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = env::args().skip(1);
    let model_dir = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: nemotron_bench <model_dir> <wav_16k_mono_pcm> [lang_slot] [fp16|int8]")?;
    let wav = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: nemotron_bench <model_dir> <wav_16k_mono_pcm> [lang_slot] [fp16|int8]")?;
    let lang_slot = args
        .next()
        .as_deref()
        .unwrap_or("0")
        .parse::<i64>()
        .map_err(|e| format!("invalid lang_slot: {e}"))?;
    let variant = args.next().unwrap_or_else(|| {
        model_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    });
    let cpu_capable = !variant.contains("int8");

    let samples = read_wav_16k_mono_pcm(&wav)?;
    let secs = samples.len() as f64 / 16_000.0;
    println!(
        "bench: model={} wav={} samples={} ({secs:.2}s) lang_slot={} variant={}",
        model_dir.display(),
        wav.display(),
        samples.len(),
        lang_slot,
        if cpu_capable { "fp16" } else { "int8" }
    );

    let mut model = NemotronModel::new(&model_dir, cpu_capable)?;
    let start = Instant::now();
    let text = model.transcribe_samples(samples, lang_slot)?;
    let elapsed = start.elapsed().as_secs_f64();
    println!("bench: elapsed={elapsed:.3}s rtf={:.3}", elapsed / secs.max(0.001));
    println!("bench: transcript={}", text.trim());
    Ok(())
}

fn read_wav_16k_mono_pcm(path: &PathBuf) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let data = fs::read(path)?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!("{} is not a RIFF/WAVE file", path.display()).into());
    }

    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pcm: Option<&[u8]> = None;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?) as usize;
        pos += 8;
        if pos + len > data.len() {
            return Err(format!("malformed WAV chunk in {}", path.display()).into());
        }
        match id {
            b"fmt " if len >= 16 => {
                let audio_format = u16::from_le_bytes(data[pos..pos + 2].try_into()?);
                let channels = u16::from_le_bytes(data[pos + 2..pos + 4].try_into()?);
                let sample_rate = u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?);
                let bits = u16::from_le_bytes(data[pos + 14..pos + 16].try_into()?);
                fmt = Some((audio_format, channels, sample_rate, bits));
            }
            b"data" => {
                pcm = Some(&data[pos..pos + len]);
            }
            _ => {}
        }
        pos += len + (len % 2);
    }

    let (audio_format, channels, sample_rate, bits) =
        fmt.ok_or_else(|| format!("{} has no fmt chunk", path.display()))?;
    if audio_format != 1 || channels != 1 || sample_rate != 16_000 || bits != 16 {
        return Err(format!(
            "{} must be 16 kHz mono signed 16-bit PCM (format={audio_format}, channels={channels}, rate={sample_rate}, bits={bits})",
            path.display()
        )
        .into());
    }
    let pcm = pcm.ok_or_else(|| format!("{} has no data chunk", path.display()))?;
    if pcm.len() % 2 != 0 {
        return Err(format!("{} has odd PCM byte length", path.display()).into());
    }
    Ok(pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}
