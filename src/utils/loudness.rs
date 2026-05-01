use crate::utils::config::get_config_path;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ebur128::{EbuR128, Mode};
use rodio::{Decoder, Source};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ANALYSIS_CHUNK_SAMPLES: usize = 16_384;

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct LoudnessCache {
    pub entries: HashMap<String, LoudnessCacheEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoudnessCacheEntry {
    pub modified_unix_secs: u64,
    pub file_size: u64,
    pub lufs: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct FileLoudnessMetadata {
    pub modified_unix_secs: u64,
    pub file_size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceCalibrationResult {
    pub lufs: f64,
    pub peak_dbfs: f64,
    pub samples_captured: usize,
    pub device_name: Option<String>,
}

impl LoudnessCacheEntry {
    pub fn from_metadata(metadata: FileLoudnessMetadata, lufs: Option<f64>) -> Self {
        Self {
            modified_unix_secs: metadata.modified_unix_secs,
            file_size: metadata.file_size,
            lufs,
        }
    }

    pub fn matches(&self, metadata: &FileLoudnessMetadata) -> bool {
        self.modified_unix_secs == metadata.modified_unix_secs
            && self.file_size == metadata.file_size
    }
}

pub fn load_loudness_cache() -> Result<LoudnessCache, Box<dyn Error>> {
    let path = loudness_cache_path()?;
    if !path.exists() {
        return Ok(LoudnessCache::default());
    }

    let bytes = fs::read(path)?;
    match serde_json::from_slice(&bytes) {
        Ok(cache) => Ok(cache),
        Err(_) => Ok(LoudnessCache::default()),
    }
}

pub fn save_loudness_cache(cache: &LoudnessCache) -> Result<(), Box<dyn Error>> {
    let path = loudness_cache_path()?;
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        fs::create_dir_all(dir)?;
    }

    let json = serde_json::to_string_pretty(cache)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn get_file_loudness_metadata(path: &Path) -> Result<FileLoudnessMetadata, Box<dyn Error>> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let modified_unix_secs = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    Ok(FileLoudnessMetadata {
        modified_unix_secs,
        file_size: metadata.len(),
    })
}

pub fn analyze_audio_file(path: &Path) -> Result<f64, Box<dyn Error + Send + Sync>> {
    let file = fs::File::open(path)?;
    let mut decoder = Decoder::try_from(file)?;
    let channels = decoder.channels().get() as u32;
    let sample_rate = decoder.sample_rate().get();

    let mut meter = EbuR128::new(channels, sample_rate, Mode::I | Mode::HISTOGRAM)?;
    let mut chunk = Vec::with_capacity(ANALYSIS_CHUNK_SAMPLES);

    for sample in decoder.by_ref() {
        chunk.push(sample);
        if chunk.len() >= ANALYSIS_CHUNK_SAMPLES {
            meter.add_frames_f32(&chunk)?;
            chunk.clear();
        }
    }

    if !chunk.is_empty() {
        meter.add_frames_f32(&chunk)?;
    }

    Ok(meter.loudness_global()?)
}

pub fn list_capture_sources() -> Result<Vec<String>, Box<dyn Error>> {
    let host = capture_host();
    let mut devices = vec![];

    for device in host.input_devices()? {
        if let Ok(description) = device.description() {
            devices.push(description.name().to_string());
        }
    }

    devices.sort();
    devices.dedup();
    Ok(devices)
}

pub fn calibrate_voice_capture(
    device_name: Option<&str>,
    duration_secs: u32,
) -> Result<VoiceCalibrationResult, Box<dyn Error>> {
    calibrate_voice_capture_inner(device_name, |stream| {
        stream.play()?;
        std::thread::sleep(Duration::from_secs(duration_secs as u64));
        Ok(())
    })
}

pub fn calibrate_voice_capture_until_stopped(
    device_name: Option<&str>,
    stop_requested: Arc<AtomicBool>,
) -> Result<VoiceCalibrationResult, Box<dyn Error>> {
    stop_requested.store(false, Ordering::SeqCst);
    calibrate_voice_capture_inner(device_name, |stream| {
        stream.play()?;
        while !stop_requested.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })
}

fn calibrate_voice_capture_inner(
    device_name: Option<&str>,
    run_stream: impl FnOnce(&cpal::Stream) -> Result<(), Box<dyn Error>>,
) -> Result<VoiceCalibrationResult, Box<dyn Error>> {
    let host = capture_host();
    let device = select_input_device(&host, device_name)?;
    let device_name = device
        .description()
        .ok()
        .map(|description| description.name().to_string());
    let supported_config = device.default_input_config()?;
    let sample_format = supported_config.sample_format();
    let stream_config: cpal::StreamConfig = supported_config.clone().into();

    let meter = Arc::new(Mutex::new(CaptureMeter::new(
        stream_config.channels as u32,
        stream_config.sample_rate,
    )?));
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_input_stream_f32(&device, &stream_config, meter.clone())?,
        cpal::SampleFormat::I16 => build_input_stream_i16(&device, &stream_config, meter.clone())?,
        cpal::SampleFormat::U16 => build_input_stream_u16(&device, &stream_config, meter.clone())?,
        other => return Err(format!("Unsupported capture format: {:?}", other).into()),
    };

    run_stream(&stream)?;
    drop(stream);

    let meter = meter.lock().unwrap_or_else(|err| err.into_inner());
    if meter.samples_captured == 0 {
        return Err("No microphone samples were captured".into());
    }

    Ok(VoiceCalibrationResult {
        lufs: meter.meter.loudness_global()?,
        peak_dbfs: linear_to_dbfs(meter.peak_linear),
        samples_captured: meter.samples_captured,
        device_name,
    })
}

fn capture_host() -> cpal::Host {
    for host_id in [cpal::HostId::PipeWire, cpal::HostId::PulseAudio] {
        if cpal::available_hosts().contains(&host_id)
            && let Ok(host) = cpal::host_from_id(host_id)
        {
            return host;
        }
    }

    cpal::default_host()
}

fn loudness_cache_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(get_config_path()?.join("loudness-cache.json"))
}

fn select_input_device(
    host: &cpal::Host,
    device_name: Option<&str>,
) -> Result<cpal::Device, Box<dyn Error>> {
    if let Some(device_name) = device_name {
        for device in host.input_devices()? {
            let Ok(description) = device.description() else {
                continue;
            };
            let name = description.name();
            if name == device_name || name.contains(device_name) {
                return Ok(device);
            }
        }
        return Err(format!("Input device not found: {}", device_name).into());
    }

    host.default_input_device()
        .ok_or_else(|| "No default input device available".into())
}

struct CaptureMeter {
    meter: EbuR128,
    peak_linear: f32,
    samples_captured: usize,
}

impl CaptureMeter {
    fn new(channels: u32, sample_rate: u32) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            meter: EbuR128::new(channels, sample_rate, Mode::I | Mode::HISTOGRAM)?,
            peak_linear: 0.0,
            samples_captured: 0,
        })
    }
}

fn build_input_stream_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: Arc<Mutex<CaptureMeter>>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let channels = config.channels as usize;
    let err_fn = |err| eprintln!("Voice calibration stream error: {}", err);
    let stream = device.build_input_stream(
        *config,
        move |data: &[f32], _| process_capture_frames_f32(data, channels, &meter),
        err_fn,
        None,
    )?;
    Ok(stream)
}

fn build_input_stream_i16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: Arc<Mutex<CaptureMeter>>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let channels = config.channels as usize;
    let err_fn = |err| eprintln!("Voice calibration stream error: {}", err);
    let stream = device.build_input_stream(
        *config,
        move |data: &[i16], _| {
            let interleaved: Vec<f32> = data
                .iter()
                .map(|sample| *sample as f32 / i16::MAX as f32)
                .collect();
            process_capture_frames_f32(&interleaved, channels, &meter);
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

fn build_input_stream_u16(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: Arc<Mutex<CaptureMeter>>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let channels = config.channels as usize;
    let err_fn = |err| eprintln!("Voice calibration stream error: {}", err);
    let stream = device.build_input_stream(
        *config,
        move |data: &[u16], _| {
            let interleaved: Vec<f32> = data
                .iter()
                .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                .collect();
            process_capture_frames_f32(&interleaved, channels, &meter);
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

fn process_capture_frames_f32(
    interleaved: &[f32],
    channels: usize,
    meter: &Arc<Mutex<CaptureMeter>>,
) {
    let peak = interleaved
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max);

    let mut meter = meter.lock().unwrap_or_else(|err| err.into_inner());
    meter.peak_linear = meter.peak_linear.max(peak);
    meter.samples_captured += interleaved.len() / channels;
    meter.meter.add_frames_f32(interleaved).ok();
}

fn linear_to_dbfs(value: f32) -> f64 {
    if value <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * f64::from(value).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileLoudnessMetadata, LoudnessCacheEntry};

    #[test]
    fn loudness_cache_entry_matches_same_file_metadata() {
        let metadata = FileLoudnessMetadata {
            modified_unix_secs: 10,
            file_size: 2048,
        };
        let entry = LoudnessCacheEntry::from_metadata(metadata, Some(-18.0));

        assert!(entry.matches(&metadata));
    }

    #[test]
    fn loudness_cache_entry_rejects_changed_file_metadata() {
        let entry = LoudnessCacheEntry::from_metadata(
            FileLoudnessMetadata {
                modified_unix_secs: 10,
                file_size: 2048,
            },
            Some(-18.0),
        );

        assert!(!entry.matches(&FileLoudnessMetadata {
            modified_unix_secs: 11,
            file_size: 2048,
        }));
        assert!(!entry.matches(&FileLoudnessMetadata {
            modified_unix_secs: 10,
            file_size: 4096,
        }));
    }
}
