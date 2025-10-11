// StreamTune - Lightweight Music Player/Manager
// Copyright (C) 2025 DeMoD LLC
//
// This library is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this library. If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Result, Context};
use prost::Message;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::Path;
use std::sync::Mutex;
use symphonia::core::{
    audio::{SampleBuffer, SignalSpec},
    codecs::{Decoder, DecoderOptions},
    formats::{FormatOptions, FormatReader},
    meta::MetadataOptions,
    probe::Hint,
    io::MediaSourceStream,
    errors::Error as SymphoniaError,
};
use libopusenc::{
    OpusEncoder, OpusEncComments, OpusEncApplication, OpusEncBandwidth, OpusEncBitrate,
    OpusEncChannelMapping, OpusEncSampleRate,
};
use fundsp::prelude::*;
use rodio::{Decoder as RodioDecoder, Source};
use tracing::{info, error};
use tracing_subscriber;
use streamdb::{StreamDB, StreamDBError};

mod metadata;
use metadata::{Metadata, Playlist, EqConfig};

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("File not found or inaccessible: {0}")]
    FileNotFound(String),
    #[error("Failed to decode audio: {0}")]
    DecodeError(String),
    #[error("Failed to encode audio: {0}")]
    EncodeError(String),
    #[error("StreamDB operation failed: {0}")]
    StreamDBError(String),
    #[error("DSP processing failed: {0}")]
    DSPError(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("IO error: {0}")]
    IOError(String),
}

#[derive(Clone, Debug)]
pub enum Effect {
    Delay(f64),
    Reverb(f64),
    Chorus(f64),
    Eq { bass: f64, mid: f64, treble: f64 },
    None,
}

pub struct DSPChain {
    graph: An<impl AudioUnit64>,
    sample_rate: u32,
}

impl DSPChain {
    pub fn new(sample_rate: u32, effects: Vec<Effect>) -> Result<Self, AppError> {
        let mut graph = dc(0.0);
        for effect in effects {
            graph = match effect {
                Effect::Delay(time) if time >= 0.0 && time <= 2.0 => graph >> comb(1.0, time, 0.5),
                Effect::Reverb(mix) if mix >= 0.0 && mix <= 1.0 => allpass(0.7, 0.05) >> allpass(0.8, 0.03) * mix + graph * (1.0 - mix),
                Effect::Chorus(rate) if rate >= 0.1 && rate <= 10.0 => graph >> lfo_sine(rate) * 0.01 + graph,
                Effect::Eq { bass, mid, treble } => {
                    if bass.abs() > 12.0 || mid.abs() > 12.0 || treble.abs() > 12.0 {
                        return Err(AppError::InvalidInput("EQ gains must be between -12dB and +12dB".to_string()));
                    }
                    graph >> lowshelf_hz(100.0, 1.0, bass) >> bell_hz(1000.0, 1.0, mid) >> highshelf_hz(5000.0, 1.0, treble)
                }
                _ => return Err(AppError::InvalidInput("Invalid effect parameters".to_string())),
            };
        }
        Ok(Self { graph: graph.into(), sample_rate })
    }

    pub fn process(&self, input: &[f64], output: &mut [f64]) -> Result<(), AppError> {
        if input.len() != output.len() {
            return Err(AppError::DSPError("Input and output buffer sizes mismatch".to_string()));
        }
        self.graph.lock()
            .map_err(|e| AppError::DSPError(format!("Failed to lock DSP graph: {}", e)))?
            .process(input.len() / 2, input, output);
        Ok(())
    }
}

pub struct AppState {
    db: StreamDB,
}

impl AppState {
    pub fn new(db_path: String) -> Result<Self, AppError> {
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{}.log", db_path))
            .map_err(|e| AppError::IOError(format!("Failed to open log file: {}", e)))?;
        tracing_subscriber::fmt()
            .with_writer(Mutex::new(log_file))
            .with_env_filter("info")
            .init();
        info!("Initialized AppState with db_path: {}", db_path);

        let db = StreamDB::open_with_config(db_path, streamdb::Config::default())
            .map_err(|e| AppError::StreamDBError(format!("Failed to init StreamDB: {}", e)))?;
        Ok(Self { db })
    }

    pub async fn add_track(&mut self, path: String) -> Result<Metadata, AppError> {
        info!("Adding track: {}", path);
        let file = File::open(&path)
            .map_err(|e| AppError::FileNotFound(format!("Cannot open file {}: {}", path, e)))?;
        let mss = MediaSourceStream::new(Box::new(BufReader::new(file)), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = Path::new(&path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = get_probe()
            .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
            .map_err(|e| AppError::DecodeError(format!("Failed to probe file: {}", e)))?;
        let mut format = probed.format;
        let track = format.tracks().first()
            .ok_or_else(|| AppError::DecodeError("No tracks found in file".to_string()))?;
        let track_id = track.id;
        let mut decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| AppError::DecodeError(format!("Failed to create decoder: {}", e)))?;

        let tags = format.metadata().current()
            .map(|m| m.tags().iter().cloned().collect::<std::collections::HashMap<_, _>>())
            .unwrap_or_default();
        let sample_rate = track.codec_params.sample_rate.unwrap_or(48000);
        let channels = track.codec_params.channels.map_or(2, |c| c.count() as u16);
        let duration_secs = track.codec_params.n_frames
            .map_or(0.0, |f| f as f64 / sample_rate as f64);

        let metadata = Metadata {
            title: tags.get("TITLE").map(|v| v.value.to_string()).unwrap_or("Unknown".to_string()),
            artist: tags.get("ARTIST").map(|v| v.value.to_string()).unwrap_or("Unknown".to_string()),
            album: tags.get("ALBUM").map(|v| v.value.to_string()).unwrap_or("Unknown".to_string()),
            duration: duration_secs,
        };
        let mut metadata_bytes = Vec::new();
        metadata.encode_length_delimited(&mut metadata_bytes)
            .map_err(|e| AppError::InvalidInput(format!("Failed to encode metadata: {}", e)))?;

        let mut comments = OpusEncComments::create()
            .map_err(|e| AppError::EncodeError(format!("Failed to create Opus comments: {}", e)))?;
        comments.add("TITLE", &metadata.title)
            .and_then(|_| comments.add("ARTIST", &metadata.artist))
            .and_then(|_| comments.add("ALBUM", &metadata.album))
            .map_err(|e| AppError::EncodeError(format!("Failed to add Opus comments: {}", e)))?;

        let mut encoded_data: Vec<u8> = Vec::new();
        let write_callback = |data: &mut Vec<u8>, buffer: &[u8]| -> bool {
            data.extend_from_slice(buffer); true
        };
        let close_callback = |_data: &mut Vec<u8>| -> bool { true };

        let opus_sample_rate = match sample_rate {
            8000 => OpusEncSampleRate::Hz8000, 12000 => OpusEncSampleRate::Hz12000,
            16000 => OpusEncSampleRate::Hz16000, 24000 => OpusEncSampleRate::Hz24000,
            _ => OpusEncSampleRate::Hz48000,
        };
        let frame_size = match opus_sample_rate {
            OpusEncSampleRate::Hz48000 => 960, OpusEncSampleRate::Hz24000 => 480,
            OpusEncSampleRate::Hz16000 => 320, OpusEncSampleRate::Hz12000 => 240,
            OpusEncSampleRate::Hz8000 => 160,
        };
        let channel_mapping = if channels == 1 { OpusEncChannelMapping::MonoStereo } else { OpusEncChannelMapping::MonoStereo };

        let mut encoder = OpusEncoder::create_callbacks(
            write_callback, close_callback, Some(&mut encoded_data), &mut comments,
            opus_sample_rate, channels as u8, channel_mapping,
        ).map_err(|e| AppError::EncodeError(format!("Failed to create Opus encoder: {}", e)))?;
        encoder.set_application(OpusEncApplication::Audio)
            .and_then(|_| encoder.set_bitrate(OpusEncBitrate::Explicit(128000)))
            .and_then(|_| encoder.set_vbr(true))
            .and_then(|_| encoder.set_complexity(10))
            .and_then(|_| encoder.set_bandwidth(OpusEncBandwidth::Fullband24kHz))
            .map_err(|e| AppError::EncodeError(format!("Failed to configure Opus encoder: {}", e)))?;

        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(_)) => break,
                Err(e) => {
                    error!("Decode error on {}: {}", path, e);
                    return Err(AppError::DecodeError(format!("Failed to decode packet: {}", e)));
                }
            };
            if packet.track_id() != track_id { continue; }
            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let mut sample_buf = SampleBuffer::<i16>::new(decoded.capacity() as u32, spec);
                    sample_buf.copy_interleaved_ref(decoded);
                    let samples = sample_buf.samples();
                    for chunk in samples.chunks(frame_size * channels as usize) {
                        encoder.write(chunk, chunk.len() / channels as usize)
                            .map_err(|e| AppError::EncodeError(format!("Failed to encode chunk: {}", e)))?;
                    }
                }
                Err(SymphoniaError::IoError(_)) => break,
                Err(e) => {
                    error!("Decode error on {}: {}", path, e);
                    return Err(AppError::DecodeError(format!("Failed to decode: {}", e)));
                }
            }
        }
        encoder.drain()
            .map_err(|e| AppError::EncodeError(format!("Failed to drain encoder: {}", e)))?;

        let key = format!("audio:{}:{}:{}", metadata.artist, metadata.album, metadata.title);
        let mut value = metadata_bytes;
        value.extend_from_slice(&encoded_data);
        self.db.insert(key.as_bytes(), &value, None)
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
        info!("Successfully added track: {}", key);
        Ok(metadata)
    }

    pub fn create_playlist(&mut self, name: String) -> Result<String, AppError> {
        if name.is_empty() {
            return Err(AppError::InvalidInput("Playlist name cannot be empty".to_string()));
        }
        let key = format!("playlist:{}", name);
        let playlist = Playlist { track_keys: vec![] };
        let mut value = Vec::new();
        playlist.encode_length_delimited(&mut value)
            .map_err(|e| AppError::InvalidInput(format!("Failed to encode playlist: {}", e)))?;
        self.db.insert(key.as_bytes(), &value, None)
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
        info!("Created playlist: {}", key);
        Ok(key)
    }

    pub fn add_to_playlist(&mut self, playlist_key: String, track_key: String) -> Result<(), AppError> {
        let value = self.db.get(playlist_key.as_bytes())
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?
            .ok_or_else(|| AppError::InvalidInput(format!("Playlist not found: {}", playlist_key)))?;
        let mut playlist = Playlist::decode_length_delimited(&value[..])
            .map_err(|e| AppError::InvalidInput(format!("Failed to decode playlist: {}", e)))?;
        if !track_key.starts_with("audio:") {
            return Err(AppError::InvalidInput(format!("Invalid track key: {}", track_key)));
        }
        playlist.track_keys.push(track_key.clone());
        let mut new_value = Vec::new();
        playlist.encode_length_delimited(&mut new_value)
            .map_err(|e| AppError::InvalidInput(format!("Failed to encode playlist: {}", e)))?;
        self.db.insert(playlist_key.as_bytes(), &new_value, None)
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
        info!("Added track {} to playlist {}", track_key, playlist_key);
        Ok(())
    }

    pub fn get_playlist(&self, playlist_key: String) -> Result<Vec<Metadata>, AppError> {
        let value = self.db.get(playlist_key.as_bytes())
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?
            .ok_or_else(|| AppError::InvalidInput(format!("Playlist not found: {}", playlist_key)))?;
        let playlist = Playlist::decode_length_delimited(&value[..])
            .map_err(|e| AppError::InvalidInput(format!("Failed to decode playlist: {}", e)))?;
        let mut tracks = Vec::new();
        for key in playlist.track_keys {
            match self.db.get(key.as_bytes()) {
                Ok(Some(data)) => {
                    let metadata = Metadata::decode_length_delimited(&mut &data[..])
                        .map_err(|e| AppError::InvalidInput(format!("Failed to decode metadata for {}: {}", key, e)))?;
                    tracks.push(metadata);
                }
                Ok(None) => {
                    error!("Track not found: {}", key);
                    continue;
                }
                Err(e) => {
                    error!("Error accessing track {}: {}", key, e);
                    return Err(AppError::StreamDBError(format!("Failed to access track {}: {}", key, e)));
                }
            }
        }
        Ok(tracks)
    }

    pub fn get_track_audio(&self, key: String) -> Result<Vec<u8>, AppError> {
        let data = self.db.get(key.as_bytes())
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?
            .ok_or_else(|| AppError::InvalidInput(format!("Track not found: {}", key)))?;
        let metadata = Metadata::decode_length_delimited(&mut &data[..])
            .map_err(|e| AppError::InvalidInput(format!("Failed to decode metadata: {}", e)))?;
        let metadata_len = metadata.encoded_len();
        Ok(data[metadata_len..].to_vec())
    }

    pub fn search_tracks(&self, prefix: String) -> Result<Vec<Metadata>, AppError> {
        if prefix.is_empty() {
            return Err(AppError::InvalidInput("Search prefix cannot be empty".to_string()));
        }
        let keys = self.db.prefix_search(prefix.as_bytes())
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
        let mut tracks = Vec::new();
        for key in keys {
            match self.db.get(&key) {
                Ok(Some(data)) => {
                    let metadata = Metadata::decode_length_delimited(&mut &data[..])
                        .map_err(|e| AppError::InvalidInput(format!("Failed to decode metadata for {}: {}", String::from_utf8_lossy(&key), e)))?;
                    tracks.push(metadata);
                }
                Ok(None) => {
                    error!("Track not found for key: {}", String::from_utf8_lossy(&key));
                    continue;
                }
                Err(e) => {
                    error!("Error accessing track {}: {}", String::from_utf8_lossy(&key), e);
                    return Err(AppError::StreamDBError(format!("Failed to access track: {}", e)));
                }
            }
        }
        Ok(tracks)
    }

    pub fn apply_dsp_to_track(&self, key: String, mut effects: Vec<Effect>) -> Result<Vec<u8>, AppError> {
        if let Some(config_data) = self.db.get("config:eq".as_bytes())
            .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))? {
            let eq_config = EqConfig::decode_length_delimited(&config_data[..])
                .map_err(|e| AppError::InvalidInput(format!("Failed to decode EQ config: {}", e)))?;
            effects.push(Effect::Eq {
                bass: eq_config.bass,
                mid: eq_config.mid,
                treble: eq_config.treble,
            });
        }

        let opus_data = self.get_track_audio(key.clone())?;
        let cursor = std::io::Cursor::new(opus_data);
        let source = RodioDecoder::new_opus(cursor)
            .map_err(|e| AppError::DSPError(format!("Failed to decode Opus for DSP: {}", e)))?;

        let channels = source.channels() as usize;
        let sample_rate = source.sample_rate();
        let dsp = DSPChain::new(sample_rate, effects)?;
        let mut encoded_data: Vec<u8> = Vec::new();
        let write_callback = |data: &mut Vec<u8>, buffer: &[u8]| -> bool { data.extend_from_slice(buffer); true };
        let close_callback = |_data: &mut Vec<u8>| -> bool { true };

        let mut comments = OpusEncComments::create()
            .map_err(|e| AppError::EncodeError(format!("Failed to create Opus comments for DSP: {}", e)))?;
        let metadata = self.get_playlist(key.clone())?.into_iter().next()
            .ok_or_else(|| AppError::InvalidInput(format!("Metadata not found for track: {}", key)))?;
        comments.add("TITLE", &metadata.title)
            .and_then(|_| comments.add("ARTIST", &metadata.artist))
            .and_then(|_| comments.add("ALBUM", &metadata.album))
            .map_err(|e| AppError::EncodeError(format!("Failed to add Opus comments for DSP: {}", e)))?;

        let mut encoder = OpusEncoder::create_callbacks(
            write_callback, close_callback, Some(&mut encoded_data), &mut comments,
            OpusEncSampleRate::Hz48000, channels as u8, OpusEncChannelMapping::MonoStereo,
        ).map_err(|e| AppError::EncodeError(format!("Failed to create DSP Opus encoder: {}", e)))?;
        encoder.set_application(OpusEncApplication::Audio)
            .and_then(|_| encoder.set_bitrate(OpusEncBitrate::Explicit(128000)))
            .and_then(|_| encoder.set_vbr(true))
            .and_then(|_| encoder.set_complexity(10))
            .and_then(|_| encoder.set_bandwidth(OpusEncBandwidth::Fullband24kHz))
            .map_err(|e| AppError::EncodeError(format!("Failed to configure DSP Opus encoder: {}", e)))?;

        let frame_size = 960; // 20ms at 48kHz
        let mut input_buffer: Vec<f64> = Vec::new();
        let mut output_buffer: Vec<f64> = vec![0.0; frame_size * channels];
        for sample in source {
            if let Ok(s) = sample {
                input_buffer.push(s as f64);
                if input_buffer.len() >= frame_size * channels {
                    dsp.process(&input_buffer, &mut output_buffer)?;
                    let chunk_i16: Vec<i16> = output_buffer.iter().map(|&x| x as i16).collect();
                    encoder.write(&chunk_i16, chunk_i16.len() / channels)
                        .map_err(|e| AppError::EncodeError(format!("Failed to encode DSP chunk: {}", e)))?;
                    input_buffer.clear();
                }
            }
        }
        if !input_buffer.is_empty() {
            output_buffer.resize(input_buffer.len(), 0.0);
            dsp.process(&input_buffer, &mut output_buffer)?;
            let chunk_i16: Vec<i16> = output_buffer.iter().map(|&x| x as i16).collect();
            encoder.write(&chunk_i16, chunk_i16.len() / channels)
                .map_err(|e| AppError::EncodeError(format!("Failed to encode final DSP chunk: {}", e)))?;
        }
        encoder.drain()
            .map_err(|e| AppError::EncodeError(format!("Failed to drain DSP encoder: {}", e)))?;

        Ok(encoded_data)
    }

    pub fn toggle_beta_dsp(&mut self, enabled: bool, effects: Vec<Effect>) -> Result<(), AppError> {
        let config_key = "config:dsp".to_string();
        if enabled {
            let value = bincode::serialize(&effects)
                .map_err(|e| AppError::InvalidInput(format!("Failed to serialize DSP config: {}", e)))?;
            self.db.insert(config_key.as_bytes(), &value, None)
                .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
            info!("Enabled DSP with {} effects", effects.len());
        } else {
            self.db.delete(config_key.as_bytes())
                .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
            info!("Disabled DSP");
        }
        Ok(())
    }

    pub fn toggle_beta_eq(&mut self, enabled: bool, bass: f64, mid: f64, treble: f64) -> Result<(), AppError> {
        let config_key = "config:eq".to_string();
        if enabled {
            if bass.abs() > 12.0 || mid.abs() > 12.0 || treble.abs() > 12.0 {
                return Err(AppError::InvalidInput("EQ gains must be between -12dB and +12dB".to_string()));
            }
            let config = EqConfig { bass, mid, treble };
            let mut value = Vec::new();
            config.encode_length_delimited(&mut value)
                .map_err(|e| AppError::InvalidInput(format!("Failed to encode EQ config: {}", e)))?;
            self.db.insert(config_key.as_bytes(), &value, None)
                .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
            info!("Enabled EQ with gains: bass={}, mid={}, treble={}", bass, mid, treble);
        } else {
            self.db.delete(config_key.as_bytes())
                .map_err(|e: StreamDBError| AppError::StreamDBError(e.to_string()))?;
            info!("Disabled EQ");
        }
        Ok(())
    }

    pub fn get_eq_config(&self) -> Result<Option<EqConfig>, AppError> {
        match self.db.get("config:eq".as_bytes()) {
            Ok(Some(data)) => {
                let config = EqConfig::decode_length_delimited(&data[..])
                    .map_err(|e| AppError::InvalidInput(format!("Failed to decode EQ config: {}", e)))?;
                Ok(Some(config))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AppError::StreamDBError(format!("Failed to access EQ config: {}", e))),
        }
    }
}
