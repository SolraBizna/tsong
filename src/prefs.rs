//! This module is in charge of storing the user preferences values, i.e. the
//! things that are stored in `Tsong.conf` and changed in the "Preferences"
//! window.

use log::trace;
use lazy_static::lazy_static;
use crate::config;
use toml::Value;
use serde::Deserialize;

use std::{
    convert::TryInto,
    io::{Read, Write},
    sync::RwLock,
};

use portaudio::{
    HostApiIndex,
    PortAudio,
};

#[derive(Debug,Deserialize)]
pub struct Preferences {
    #[serde(default = "get_standard_volume")]
    volume: i32,
    #[serde(default)]
    show_decibels_on_volume_slider: bool,
    #[serde(default)]
    music_paths: Vec<String>,
    #[serde(default = "get_standard_desired_latency")]
    desired_latency: f64,
    #[serde(default = "get_standard_decode_ahead")]
    decode_ahead: f64,
    // these two must both match in order for the choice to be considered valid
    #[serde(default)]
    audio_api_index: Option<u32>,
    #[serde(default)]
    audio_api_name: Option<String>,
    // same
    #[serde(default)]
    audio_dev_index: Option<u32>,
    #[serde(default)]
    audio_dev_name: Option<String>,
}

const PREFS_FILE_NAME: &str = "Tsong.toml";

/// The lowest permitted volume level.
pub const MIN_VOLUME: i32 = 0;
/// The standard volume level.
pub const STANDARD_VOLUME: i32 = 100;
/// The highest permitted volume level.
pub const MAX_VOLUME: i32 = 200;

fn get_standard_volume() -> i32 { STANDARD_VOLUME }

/// The lowest permitted target latency.
pub const MIN_DESIRED_LATENCY: f64 = 0.1;
/// The standard target latency.
pub const STANDARD_DESIRED_LATENCY: f64 = 0.15;
/// The highest permitted target latency.
pub const MAX_DESIRED_LATENCY: f64 = 3.0;

fn get_standard_desired_latency() -> f64 { STANDARD_DESIRED_LATENCY }

/// The lowest permitted decode-ahead.
pub const MIN_DECODE_AHEAD: f64 = 0.5;
/// The standard decode-ahead.
pub const STANDARD_DECODE_AHEAD: f64 = 3.0;
/// The highest permitted decode-ahead.
pub const MAX_DECODE_AHEAD: f64 = 35.0;

fn get_standard_decode_ahead() -> f64 { STANDARD_DECODE_AHEAD }

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            volume: STANDARD_VOLUME,
            show_decibels_on_volume_slider: false,
            music_paths: Vec::new(),
            desired_latency: STANDARD_DESIRED_LATENCY,
            decode_ahead: STANDARD_DECODE_AHEAD,
            audio_api_index: None, audio_api_name: None,
            audio_dev_index: None, audio_dev_name: None,
        }
    }
}

lazy_static! {
    static ref PREFERENCES: RwLock<Preferences>
        = RwLock::new(Default::default());
}

/// Call at least once, at startup. This will read in saved values for the
/// preferences.
pub fn read() -> anyhow::Result<()> {
    trace!("Reading prefs.");
    let mut f = match config::open_best_for_read(PREFS_FILE_NAME)? {
        Some(f) => f,
        None => {
            *PREFERENCES.write().unwrap() = Default::default();
            return Ok(())
        },
    };
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    drop(f);
    let mut prefs = PREFERENCES.write().unwrap();
    *prefs = toml::from_str(&buf[..])?;
    prefs.desired_latency = prefs.desired_latency.max(MIN_DESIRED_LATENCY)
        .min(MAX_DESIRED_LATENCY);
    prefs.decode_ahead = prefs.decode_ahead.max(MIN_DECODE_AHEAD)
        .min(MAX_DECODE_AHEAD);
    Ok(())
}

/// Call to save changes to the preferences.
pub fn write() -> anyhow::Result<()> {
    trace!("Writing prefs.");
    let prefs = PREFERENCES.read().unwrap();
    let mut f = config::open_for_write(PREFS_FILE_NAME)?;
    writeln!(f, "volume = {}", prefs.volume)?;
    writeln!(f, "show_decibels_on_volume_slider = {}",
             prefs.show_decibels_on_volume_slider)?;
    writeln!(f, "desired_latency = {}",
             Value::Float(prefs.desired_latency))?;
    writeln!(f, "decode_ahead = {}",
             Value::Float(prefs.decode_ahead))?;
    writeln!(f, "music_paths = [")?;
    for music_path in prefs.music_paths.iter() {
        writeln!(f, "  {},", Value::String(music_path.to_string()))?;
    }
    writeln!(f, "]")?;
    match (prefs.audio_api_index, prefs.audio_api_name.as_ref()) {
        (Some(index), Some(name)) => {
            write!(f, "\n\
                       # PortAudio settings\n\
                       audio_api_index = {}\n\
                       audio_api_name = {}\n", index,
                   Value::String(name.to_string()))?;
        },
        _ => (),
    }
    match (prefs.audio_dev_index, prefs.audio_dev_name.as_ref()) {
        (Some(index), Some(name)) => {
            match (prefs.audio_api_index, prefs.audio_api_name.as_ref()) {
                (Some(_), Some(_)) => (),
                _ => f.write_all(b"\n# PortAudio settings\n")?,
            }
            write!(f, "audio_dev_index = {}\n\
                       audio_dev_name = {}\n", index,
                   Value::String(name.to_string()))?;
        }
        _ => (),
    }
    f.finish()
}

/// Returns the current setting of the volume slider, bound by `MIN_VOLUME`
/// and `MAX_VOLUME`.
pub fn get_volume() -> i32 {
    PREFERENCES.read().unwrap().volume
}

/// Alters the setting of the volume slider, clamping it within `MIN_VOLUME`
/// and `MAX_VOLUME`.
pub fn set_volume(volume: i32) {
    PREFERENCES.write().unwrap().volume
        = volume.max(MIN_VOLUME).min(MAX_VOLUME)
}

/// Returns true if the user wants to see dB, false otherwise.
pub fn get_show_decibels_on_volume_slider() -> bool {
    PREFERENCES.read().unwrap().show_decibels_on_volume_slider
}

/// Alters whether the user wants to see dB.
pub fn set_show_decibels_on_volume_slider(nu: bool) {
    PREFERENCES.write().unwrap().show_decibels_on_volume_slider = nu
}

/// Returns the current target audio latency, in seconds.
pub fn get_desired_latency() -> f64 {
    PREFERENCES.read().unwrap().desired_latency
}

/// Alters the desired audio latency, clamping it within `MIN_DESIRED_LATENCY`
/// and `MAX_DESIRED_LATENCY`.
pub fn set_desired_latency(desired_latency: f64) {
    PREFERENCES.write().unwrap().desired_latency
        = desired_latency.max(MIN_DESIRED_LATENCY).min(MAX_DESIRED_LATENCY)
}

/// Returns the number of seconds to "decode ahead".
pub fn get_decode_ahead() -> f64 {
    let prefs = PREFERENCES.read().unwrap();
    prefs.decode_ahead.max(prefs.desired_latency * 3.0)
}

/// Alters the decode-ahead value, clamping it within `MIN_DECODE_AHEAD` and
/// `MAX_DECODE_AHEAD` and also to triple the desired latency value.
pub fn set_decode_ahead(decode_ahead: f64) {
    let mut prefs = PREFERENCES.write().unwrap();
    let min = MIN_DECODE_AHEAD.max(prefs.desired_latency * 3.0);
    prefs.decode_ahead
        = decode_ahead.max(min).min(MAX_DECODE_AHEAD)
}

/// Returns a copy of the list of music paths.
pub fn get_music_paths() -> Vec<String> {
    PREFERENCES.read().unwrap().music_paths.clone()
}

/// Replaces the list of music paths.
pub fn set_music_paths(music_paths: Vec<String>) {
    PREFERENCES.write().unwrap().music_paths = music_paths
}

/// Returns the `HostApiIndex` of the audio host API chosen by the user, or of
/// the default host API if the user hasn't made a choice or if the user's
/// choice could not be found.
pub fn get_chosen_audio_api(pa: &PortAudio) -> HostApiIndex {
    let prefs = PREFERENCES.read().unwrap();
    if let Some(audio_api_index) = prefs.audio_api_index
        .and_then(|x| x.try_into().ok()) {
            if let Some(info) = pa.host_api_info(audio_api_index) {
                if let Some(audio_api_name) = prefs.audio_api_name.as_ref() {
                    if info.name == audio_api_name { return audio_api_index
                                                     as HostApiIndex }
                }
            }
        }
    return pa.default_host_api().unwrap()
}

/// Returns the device index of the audio device chosen by the user, if the
/// user has made a choice AND the chosen host API index matches the user's
/// choice of host API. Returns `None` if the user hasn't made a choice, or if
/// the passed host API index doesn't match the user's choice, or if the user's
/// choice is "use the default device".
///
/// This is a PER-API device index, hence being `u32` and not `DeviceIndex`!
pub fn get_chosen_audio_device_for_api(pa: &PortAudio,
                                       host_api: HostApiIndex) -> Option<u32> {
    let chosen_api = get_chosen_audio_api(pa);
    if chosen_api != host_api { return None }
    let prefs = PREFERENCES.read().unwrap();
    if let Some(api_dev_index) = prefs.audio_dev_index {
        let audio_dev_index
            = pa.api_device_index_to_device_index(host_api,
                                                  api_dev_index as i32);
        if let Ok(audio_dev_index) = audio_dev_index {
            if let Ok(info) = pa.device_info(audio_dev_index) {
                if info.host_api == chosen_api {
                    if let Some(audio_dev_name)=prefs.audio_dev_name.as_ref() {
                        if info.name == audio_dev_name {
                            return Some(api_dev_index as u32)
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn set_chosen_audio_api_and_device(pa: &PortAudio,
                                       api_index: HostApiIndex,
                                       api_name: &str,
                                       dev: Option<(u32,&str)>) {
    let default = pa.default_host_api().unwrap();
    let mut prefs = PREFERENCES.write().unwrap();
    if api_index == default {
        prefs.audio_api_index = None;
        prefs.audio_api_name = None;
    }
    else {
        prefs.audio_api_index = Some(api_index as u32);
        prefs.audio_api_name = Some(api_name.to_owned());
    }
    match dev {
        None => {
            prefs.audio_dev_index = None;
            prefs.audio_dev_name = None;
        },
        Some((dev_index, dev_name)) => {
            prefs.audio_dev_index = Some(dev_index);
            prefs.audio_dev_name = Some(dev_name.to_owned());
        },
    }
}
