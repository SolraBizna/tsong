//! This module handles the current playback state; playing/paused, current
//! song, current playlist, etc. It also manages the actual playback device(s),
//! opening and closing and starting and stopping the stream.

use crate::*;

use log::{warn, error};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::Ordering},
    sync::mpsc::{Sender, Receiver, channel},
    time::Instant,
};

use portaudio::{
    stream::{Parameters, OutputSettings, OutputCallbackArgs},
    PortAudio,
    StreamCallbackResult,
};
use lazy_static::lazy_static;
use anyhow::anyhow;
use libsoxr::Soxr;

/// Internal state used when resampling audio. Wraps `libsoxr`.
struct ResampleState {
    input_rate: f64,
    output_rate: f64,
    channel_count: i32,
    soxr: Soxr,
}

trait ResampleStateOptionImplHack {
    fn output(&mut self, native_sample_rate: Option<f64>, frame: AudioFrame)
        -> anyhow::Result<()>;
}

impl ResampleStateOptionImplHack for Option<ResampleState> {
    fn output(&mut self, native_sample_rate: Option<f64>, frame: AudioFrame)
        -> anyhow::Result<()> {
        if let Some(native_sample_rate) = native_sample_rate {
            let need_recreate = match self {
                Some(x) => x.output_rate != native_sample_rate // never true?
                    || x.input_rate != frame.sample_rate
                    || x.channel_count != frame.channel_count,
                None => frame.sample_rate != native_sample_rate,
            };
            if need_recreate {
                if let Some(me) = self {
                    let mut buf = bufring::get_buf();
                    buf.resize(512, 0.0); // hopefully enough
                    let (_, out_floats) = me.soxr.process::<f32,f32>
                        (None, &mut buf[..])?;
                    buf.resize(out_floats * me.channel_count as usize, 0.0);
                    FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
                        song_id: frame.song_id,
                        time: frame.time,
                        sample_rate: native_sample_rate,
                        channel_count: me.channel_count,
                        data: buf,
                        consumed: 0,
                    });
                }
                if native_sample_rate == frame.sample_rate {
                    *self = None;
                }
                else {
                    let new_resampler = ResampleState {
                        input_rate: frame.sample_rate,
                        output_rate: native_sample_rate,
                        channel_count: frame.channel_count,
                        soxr:
                        Soxr::create(frame.sample_rate, native_sample_rate,
                                     frame.channel_count as u32,
                                     None, None, None)?,
                    };
                    *self = Some(new_resampler);
                }
            }
            if let Some(me) = self {
                let mut buf = bufring::get_buf();
                buf.resize((frame.data.len() as f64 * me.output_rate
                            / me.input_rate).ceil() as usize + 200, 0.0);
                let mut rem = &frame.data[..];
                let mut buf_pos = 0;
                while rem.len() > 0 {
                    let out = &mut buf[buf_pos..];
                    let (in_frames, out_frames) = me.soxr.process
                        (Some(&rem[..]), &mut out[..])?;
                    let in_floats = in_frames * me.channel_count as usize;
                    let out_floats = out_frames * me.channel_count as usize;
                    rem = &rem[in_floats..];
                    buf_pos += out_floats;
                    if rem.len() > 0 {
                        buf.resize(buf.len() * 2, 0.0);
                    }
                }
                let mut frame = frame;
                frame.data = buf;
                frame.data.resize(buf_pos, 0.0);
                frame.sample_rate = native_sample_rate;
                FRAME_QUEUE.lock().unwrap().push_back(frame);
            }
            else {
                FRAME_QUEUE.lock().unwrap().push_back(frame);
            }
        }
        else {
            FRAME_QUEUE.lock().unwrap().push_back(frame);
        }
        Ok(())
    }
}

/// A chunk of audio, ready to be sent to the sound card.
struct AudioFrame {
    song_id: SongID,
    /// time in seconds from beginning of song that this frame starts at
    time: f64,
    sample_rate: f64,
    channel_count: i32,
    data: Vec<f32>, // hooray! lots of copying!
    /// number of indices within data that have been consumed
    consumed: usize,
}

impl Drop for AudioFrame {
    fn drop(&mut self) {
        let mut buf = Vec::new();
        std::mem::swap(&mut self.data, &mut buf);
        bufring::finished_with_buf(buf);
    }
}

#[derive(Debug)]
pub enum PlaybackCommand {
    /// Start playing a song. If a song is provided, start at the beginning of
    /// the target song. If not, if playback is currently paused, resume it. If
    /// not, start playing the first song in the playlist.
    ///
    /// `Play(None)` = clicking on the Play control. `Play(some song)` =
    /// double-clicking on a song in the playlist UI.
    Play(Option<LogicalSongRef>),
    /// Pause playback. A subsequent `Play(None)` will continue playback at the
    /// last point in the song that the user heard.
    Pause,
    /// Stop playback. The current song is discarded, and a subsequent
    /// `Play(None)` will start the playlist over.
    Stop,
    /// Go to the next song in the playlist. If the playlist is over, and
    /// looping is not enabled, same as `Stop`. Otherwise, if playback is
    /// currently not active, acts as if we paused at the beginning of the next
    /// song.
    Next,
    /// Go to the previous song in the playlist. If we're at the beginning of
    /// the playlist, or we're not near the beginning of a song, starts the
    /// current song over. If playback is currently not active, acts as if we
    /// paused at the beginning of whatever song gets picked.
    Prev
}
use PlaybackCommand::*;

/// CallbackReports are tied to time stamps in the stream timebase. Each one
/// indicates that what the user is hearing will match the given report at that
/// time.
#[derive(Debug)]
enum CallbackReport {
    /// User is hearing the given point in time of the given song.
    SongPlaying { song_id: SongID, time: f64 },
    /// User has heard the end of playback, and the stream should be closed.
    PlaybackFinished,
    /// A sample format change is needed, and the stream should be closed.
    SampleFormatChanged,
}
use CallbackReport::*;

#[derive(Debug)]
enum PlaybackThreadMessage {
    Command(PlaybackCommand),
    CallbackRan,
}

#[derive(Clone,Copy,Debug,PartialEq)]
pub enum PlaybackStatus {
    /// A song is playing. The user is hearing audio.
    Playing,
    /// A song was playing, but has been paused. `Play(None)` at this point
    /// will resume the selected song.
    Paused,
    /// No playback is happening and no song is selected.
    Stopped,
}

impl Default for PlaybackStatus {
    fn default() -> PlaybackStatus { PlaybackStatus::Stopped }
}

impl PlaybackStatus {
    /// True if playback is currently happening, false if it's paused or
    /// stopped.
    pub fn is_playing(&self) -> bool {
        *self == PlaybackStatus::Playing
    }
}

#[derive(Default)]
struct InternalState {
    /// The song that the user is *currently hearing*, and the timestamp within
    /// the song that (supposedly) is reaching their ears right now.
    active_song: Option<(LogicalSongRef,f64)>,
    /// The song that the user *will* be hearing if *all buffers currently
    /// queued are played*.
    future_song: Option<LogicalSongRef>,
    /// The FFMPEG input stream corresponding to `future_song`.
    future_stream: Option<ffmpeg::AVFormat>,
    /// The playlist from which the *next* song will be drawn.
    future_playlist: Option<PlaylistRef>,
    /// The playback thread will update this to reflect the current playback
    /// state, as it changes in response to commands.
    status: PlaybackStatus,
    /// Whether we are currently muted. When we're muted, we pretend our volume
    /// is set to zero.
    muted: bool,
}

lazy_static! {
    // We can't have an `RwLock` here, because `RwLock` doesn't grant Sync (as
    // multiple readers could read simultaneously) and `AVFormat` isn't Sync.
    static ref STATE: Arc<Mutex<InternalState>>
        = Arc::new(Mutex::new(Default::default()));
    // a bit silly to use an MPSC sender like this, but oh well
    static ref PLAYBACK_CONTROL_TX: Mutex<Option<Sender<PlaybackThreadMessage>>>
        = Mutex::new(None);
    static ref FRAME_QUEUE: Mutex<VecDeque<AudioFrame>>
        = Mutex::new(VecDeque::new());
    static ref REPORT_QUEUE: Mutex<VecDeque<(f64,CallbackReport)>>
        = Mutex::new(VecDeque::new());
    static ref CURRENT_AUDIO_FORMAT: Mutex<(f64, i32)>
        = Mutex::new(Default::default());
    static ref BROKEN_STREAM_TIME: std::sync::atomic::AtomicBool
        = Default::default();
    /// used if `BROKEN_STREAM_TIME` is true
    static ref BROKEN_EPOCH: Instant = Instant::now();
}

/// Selects a different playlist to be active, without changing the active
/// song.
pub fn set_future_playlist(new_playlist: Option<PlaylistRef>) {
    let mut me = STATE.lock().unwrap();
    if me.future_playlist != new_playlist {
        me.future_playlist = new_playlist;
    }
}

/// Gets the currently active playlist, if there is one.
pub fn get_future_playlist() -> Option<PlaylistRef> {
    STATE.lock().unwrap().future_playlist.as_ref().cloned()
}

/// Returns the current playback status.
pub fn get_playback_status() -> PlaybackStatus {
    STATE.lock().unwrap().status
}

/// Returns the song that is (or would be) being pumped into the user's ears
/// at this exact moment, and the point that we are at in the song.
pub fn get_active_song() -> Option<(LogicalSongRef,f64)> {
    STATE.lock().unwrap().active_song.as_ref().cloned()
}

/// Returns the current playback status, and the active song. Only takes the
/// lock once, so the values will be coherent with one another.
pub fn get_status_and_active_song()
-> (PlaybackStatus, Option<(LogicalSongRef,f64)>) {
    let state = STATE.lock().unwrap();
    (state.status, state.active_song.as_ref().cloned())
}

pub fn send_command(wat: PlaybackCommand) {
    let mut playback_control_tx = PLAYBACK_CONTROL_TX.lock().unwrap();
    if playback_control_tx.is_none() {
        let (tx, rx) = channel();
        *playback_control_tx = Some(tx);
        let state = STATE.clone();
        std::thread::Builder::new()
            .name("Playback".to_owned())
            .spawn(move || playback_thread(state, rx))
            .unwrap();
    }
    match playback_control_tx.as_ref().unwrap().send(PlaybackThreadMessage::Command(wat)) {
        Ok(_) => (),
        Err(_) => error!("Playback thread has died. Playback is broken now. \
                          (issue #17)"),
    }
}

fn send_callback_report(when: f64, wat: CallbackReport) {
    REPORT_QUEUE.lock().unwrap().push_back((when, wat));
}

fn playback_callback(args: OutputCallbackArgs<f32>) -> StreamCallbackResult {
    // destructure parameters
    let OutputCallbackArgs {
        buffer,
        time,
        ..
    } = args;
    let mut now = if time.current == 0.0 && time.buffer_dac == 0.0 {
        let was_broken = BROKEN_STREAM_TIME.swap(true, Ordering::Release);
        let true_now = BROKEN_EPOCH.elapsed().as_secs_f64();
        if !was_broken {
            warn!("Stream time is broken on this driver! Using the wall-clock \
                   hack!");
            true_now // don't add latency, we're hopefully priming buffers
        }
        else {
            true_now + prefs::get_desired_latency()
        }
    }
    else {
        time.buffer_dac
    };
    let volume = if STATE.lock().unwrap().muted { 0.0 }
    else {
        let volume = prefs::get_volume() as f32 / 100.0;
        volume * volume
    };
    let mut rem = buffer;
    let mut queue = FRAME_QUEUE.lock().unwrap();
    let current_audio_format = *CURRENT_AUDIO_FORMAT.lock().unwrap();
    let (sample_rate, channel_count) = current_audio_format;
    while rem.len() > 0 {
        let next_el = match queue.get_mut(0) {
            None => break,
            Some(el) => el,
        };
        if (next_el.sample_rate, next_el.channel_count)!=current_audio_format {
            break
        }
        let next_data = &next_el.data[next_el.consumed..];
        send_callback_report(now, SongPlaying { song_id: next_el.song_id, time: next_el.time + (next_el.consumed / channel_count as usize) as f64 / sample_rate});
        if next_data.len() > rem.len() {
            copy_with_volume(rem, &next_data[..rem.len()], volume);
            now += (rem.len() / channel_count as usize) as f64 / sample_rate;
            next_el.consumed += rem.len();
            rem = &mut [];
        }
        else {
            copy_with_volume(&mut rem[..next_data.len()], next_data, volume);
            now += (next_data.len() / channel_count as usize) as f64 / sample_rate;
            rem = &mut rem[next_data.len()..];
            queue.pop_front();
        }
    }
    // fill rest with zeroes
    // (slice::fill isn't stable yet)
    for el in rem.iter_mut() { *el = 0.0; }
    // so. why did we stop?
    match queue.get(0) {
        None => {
            // No audio was left in the queue. We might have had a queue
            // underrun, or we might have reached the end of playback.
            // We use `try_lock()` instead of `lock()` to lock the state
            // because we still hold the queue lock; since the playback thread
            // will acquire the queue lock while holding the state lock, if we
            // try to do the reverse we could end up with deadlock.
            // If we couldn't get the lock, assume that playback is ongoing.
            // We'll play some extra silence, but that's okay.
            let playback_over = STATE.try_lock().map(|x| {
                x.status != PlaybackStatus::Playing
                    || x.future_song.is_none()
            }).unwrap_or(false);
            if playback_over {
                send_callback_report(now, PlaybackFinished);
            }
            // TODO: underrun detection
        },
        Some(x) => {
            if (x.sample_rate, x.channel_count) != current_audio_format {
                send_callback_report(now, SampleFormatChanged);
            }
            // otherwise, nothing to report
        },
    };
    let _ = PLAYBACK_CONTROL_TX.lock().unwrap().as_ref().unwrap()
        .send(PlaybackThreadMessage::CallbackRan);
    // some PA backends are buggy (including the one that ends up talking to
    // the "other" PA) and will drop buffers if we use ::Complete.
    StreamCallbackResult::Continue
}

fn copy_with_volume(dst: &mut[f32], src: &[f32], volume: f32) {
    assert_eq!(dst.len(), src.len());
    for n in 0 .. src.len() {
        dst[n] = src[n] * volume;
    }
}

fn playback_thread(state: Arc<Mutex<InternalState>>,
                   playback_control_rx: Receiver<PlaybackThreadMessage>) {
    let pa = PortAudio::new().expect("Could not initialize PortAudio");
    loop {
        while state.lock().unwrap().status != PlaybackStatus::Playing {
            match playback_control_rx.recv() {
                Err(_) => return, // bye bye...
                Ok(PlaybackThreadMessage::Command(cmd)) => {
                    match cmd {
                        Pause | Stop => (), // nothing to do
                        Play(Some(song)) => {
                            // Play the CHOSEN SONG.
                            let mut state = state.lock().unwrap();
                            state.status = PlaybackStatus::Playing;
                            state.future_stream = None;
                            state.future_song = Some(song.clone());
                            state.active_song = Some((song, 0.0));
                        },
                        Play(None) => {
                            // Play the CURRENT SONG, if there is one.
                            // Otherwise, play the FIRST SONG.
                            let mut state = state.lock().unwrap();
                            if state.future_song.is_none() {
                                state.next_song();
                                state.active_song = state.future_song
                                    .as_ref().map(|x| (x.clone(), 0.0));
                            }
                            state.status = match state.active_song {
                                Some(_) => PlaybackStatus::Playing,
                                None => PlaybackStatus::Stopped,
                            };
                        },
                        Next => {
                            // Queue the next song to be played, but don't
                            // start playing it yet.
                            let mut state = state.lock().unwrap();
                            state.next_song();
                            state.active_song = state.future_song
                                .as_ref().map(|x| (x.clone(), 0.0));
                            state.future_stream = None;
                        },
                        Prev => {
                            // Queue the previous song to be played, but don't
                            // start playing it yet.
                            //
                            // UNLESS...!
                            let mut state = state.lock().unwrap();
                            match state.active_song.as_mut() {
                                Some((_,when)) if *when >= 5.0 => {
                                    // Actually, start the current song over
                                    // instead.
                                    *when = 0.0;
                                },
                                _ => {
                                    state.prev_song();
                                    state.active_song = state.future_song
                                        .as_ref().map(|x| (x.clone(), 0.0));
                                },
                            }
                            state.future_stream = None;
                        },
                    }
                },
                Ok(_) => (), // still not playing!
            }
        }
        while state.lock().unwrap().status == PlaybackStatus::Playing {
            // One of three things has happened:
            // - We're starting playback from nothing. Make a new stream.
            // - Sample rate changed during playback. Make a new stream.
            // - User requested that a different song be played.
            // - User changed the audio device. (TODO)
            // - We hit the end of the playlist and looping isn't enabled.
            //   Finish up. (This might be handled elsewhere?)
            // But before we can do anything else, there might be some more
            // playback commands we want to handle... Yay, code duplication!
            while let Some(message) = playback_control_rx.try_recv().ok() {
                // We manually break from this loop if and only if we change
                // the playback status.
                match message {
                    // this shouldn't happen but is harmless
                    PlaybackThreadMessage::CallbackRan => (),
                    PlaybackThreadMessage::Command(cmd) => {
                        match cmd {
                            Stop => {
                                let mut state = state.lock().unwrap();
                                state.status = PlaybackStatus::Stopped;
                                state.future_song = None;
                                state.future_stream = None;
                                break;
                            },
                            Pause => {
                                let mut state = state.lock().unwrap();
                                state.status = PlaybackStatus::Paused;
                                break;
                            },
                            Play(Some(song)) => {
                                let mut state = state.lock().unwrap();
                                state.future_song = Some(song);
                                state.future_stream = None;
                            },
                            Play(None) => (), // nothing to do
                            Next => {
                                let mut state = state.lock().unwrap();
                                state.next_song();
                            },
                            Prev => {
                                let mut state = state.lock().unwrap();
                                state.future_stream = None;
                                match state.active_song.as_mut() {
                                    Some((song,when)) if *when >= 5.0 => {
                                        // Actually, start the current song
                                        // over instead.
                                        *when = 0.0;
                                        state.future_song = Some(song.clone());
                                    },
                                    _ => {
                                        state.prev_song();
                                    },
                                }
                            }
                        }
                    }
                }
            }
            errors::reset_from("Playback Thread");
            match playback_thread_inner_loop(&pa, &state,
                                             &playback_control_rx) {
                Ok(_) => (),
                Err(x) => {
                    error!("in playback thread: {}", x);
                    errors::from("Playback Thread", x.to_string());
                    let mut state = state.lock().unwrap();
                    if state.status != PlaybackStatus::Stopped {
                        state.status = PlaybackStatus::Paused;
                        if let Err(x) = state.reset_to_heard_point() {
                            error!("{}", x);
                        }
                    }
                },
            }
        }
    }
}

/// Inner loop of the playback thread. Convenient way to pass any API errors
/// upward and handle them.
fn playback_thread_inner_loop(pa: &PortAudio,
                              state: &Arc<Mutex<InternalState>>,
                              playback_control_rx:
                              &Receiver<PlaybackThreadMessage>)
    -> anyhow::Result<()> {
    // we assume that playback is happening... if it's not, go away
    if state.lock().unwrap().status != PlaybackStatus::Playing {
        return Ok(())
    }
    // Time to open a new stream...
    let hostapi_index = prefs::get_chosen_audio_api(&pa);
    let device_index = prefs::get_chosen_audio_device_for_api
        (&pa, hostapi_index);
    let device_index = match device_index {
        Some(x) => pa.api_device_index_to_device_index
            (hostapi_index, x as i32)
            .or_else(|x| Err(anyhow!("Error finding a device by index: {}", x)))?,
        None => match pa.host_api_info(hostapi_index)
            .and_then(|x| x.default_output_device) {
                Some(x) => x,
                None => pa.default_output_device()
                    .or_else(|_| Err(anyhow!("No default output device?")))?
            }
    };
    let native_sample_rate = if prefs::get_resample_audio() {
        let info = pa.device_info(device_index)?;
        if info.default_sample_rate < 1.0 { Some(44100.0) }
        else { Some(info.default_sample_rate) }
    } else { None };
    let mut resample_state = None;
    let (sample_rate, channel_count) = {
        decode_some_frames(&state, native_sample_rate, &mut resample_state);
        // double lock in the common case :(
        match FRAME_QUEUE.lock().unwrap().get(0) {
            None => {
                state.lock().unwrap().status
                    = PlaybackStatus::Stopped;
                return Ok(())
            },
            Some(ref x) =>
                (x.sample_rate, x.channel_count),
        }
    };
    *CURRENT_AUDIO_FORMAT.lock().unwrap()
        = (sample_rate, channel_count);
    let parameters = Parameters::new(device_index,
                                     channel_count,
                                     true, // interleaved
                                     prefs::get_desired_latency());
    let flags = portaudio::stream_flags
        ::PA_PRIME_OUTPUT_BUFFERS_USING_STREAM_CALLBACK;
    let settings = OutputSettings::with_flags(parameters, sample_rate,
                                              0, flags);
    let mut stream = pa.open_non_blocking_stream(settings,
                                                 playback_callback)
        .or_else(|x| Err(anyhow!("Unable to open audio stream: {}", x)))?;
    // just in case...
    REPORT_QUEUE.lock().unwrap().clear();
    BROKEN_STREAM_TIME.store(false, Ordering::Relaxed);
    decode_some_frames(&state, native_sample_rate, &mut resample_state);
    stream.start()
        .or_else(|x| Err(anyhow!("Unable to start audio stream: {}", x)))?;
    let mut sample_rate_changing = false;
    'alive_loop: while state.lock().unwrap().status == PlaybackStatus::Playing {
        let mut got_message = false;
        // process at least one message. once at least one message has
        // received, process any that are *still waiting*, then go do
        // periodic tasks.
        while let Some(message) =
            if got_message { playback_control_rx.try_recv().ok() }
        else { playback_control_rx.recv().ok() } {
            got_message = true;
            match message {
                PlaybackThreadMessage::CallbackRan => (),
                PlaybackThreadMessage::Command(cmd) => {
                    match cmd {
                        Stop => {
                            let mut state = state.lock().unwrap();
                            state.status = PlaybackStatus::Stopped;
                            state.future_song = None;
                            state.future_stream = None;
                            break 'alive_loop;
                        },
                        Pause => {
                            let mut state = state.lock().unwrap();
                            state.status = PlaybackStatus::Paused;
                            break 'alive_loop;
                        },
                        Play(Some(song)) => {
                            let mut state = state.lock().unwrap();
                            state.future_song = Some(song);
                            state.future_stream = None;
                            break 'alive_loop;
                        },
                        Play(None) => (), // nothing to do
                        Next => {
                            let mut state = state.lock().unwrap();
                            // play the next song, AS THE USER HEARS
                            state.future_stream = None;
                            state.future_song = state.active_song.as_mut().map(|(x,_)| x.clone());
                            state.next_song();
                            break 'alive_loop;
                        },
                        Prev => {
                            let mut state = state.lock().unwrap();
                            state.future_stream = None;
                            match state.active_song.as_mut() {
                                Some((song,when)) if *when >= 5.0 => {
                                    // Actually, start the current song
                                    // over instead.
                                    *when = 0.0;
                                    state.future_song = Some(song.clone());
                                },
                                _ => {
                                    state.prev_song();
                                },
                            }
                            break 'alive_loop;
                        },
                    }
                },
            }
        }
        // Now run any necessary periodic tasks, such as updating the
        // current time and song that we report.
        let now = if BROKEN_STREAM_TIME.load(Ordering::Acquire) {
            BROKEN_EPOCH.elapsed().as_secs_f64()
        } else { stream.time() };
        // temporarily take the report queue lock and...
        let mut report_queue = REPORT_QUEUE.lock().unwrap();
        while report_queue.get(0).map(|x| x.0 <= now).unwrap_or(false){
            let (report_time, el) = report_queue.pop_front().unwrap();
            match el {
                SongPlaying { song_id, time: songtime } => {
                    let mut state = state.lock().unwrap();
                    let change_song = match &state.active_song {
                        &Some(ref x) => x.0.read().unwrap()
                            .get_id() != song_id,
                        &None => true
                    };
                    let songtime = songtime + (now - report_time);
                    if change_song {
                        state.active_song = Some((logical::get_song_by_song_id(song_id).ok_or_else(|| anyhow!("Playback changed to a song not in the database!"))?, songtime));
                    }
                    else {
                        state.active_song.as_mut().unwrap().1 = songtime;
                    }
                },
                SampleFormatChanged => {
                    sample_rate_changing = true;
                    break 'alive_loop;
                },
                PlaybackFinished => {
                    let mut state = state.lock().unwrap();
                    if state.status == PlaybackStatus::Playing {
                        state.status = PlaybackStatus::Stopped;
                    }
                    break 'alive_loop;
                },
            }
        }
        // release the lock...
        drop(report_queue);
        // ...so that we're not holding it during the (expensive)
        // decoding step
        decode_some_frames(&state, native_sample_rate, &mut resample_state);
    }
    // Clean up!
    let _ = stream.abort();
    // Any reports after we decided to kill the stream are of no
    // consequence.
    REPORT_QUEUE.lock().unwrap().clear();
    // Any frames that the user was *going* to hear after they hit the
    // end of playback are of no consequence.
    if !sample_rate_changing { FRAME_QUEUE.lock().unwrap().clear() }
    let mut state = state.lock().unwrap();
    match state.status {
        PlaybackStatus::Playing => (),
        PlaybackStatus::Paused => {
            // Whatever song the user was hearing when they hit pause,
            // that's where we paused.
            state.reset_to_heard_point()?;
        },
        PlaybackStatus::Stopped => {
            // There is no longer an active song.
            state.future_song = None;
            state.active_song = None;
        },
    }
    Ok(())
}

/// Wrapper that repeatedly calls `state.decode_some_frames()` until enough
/// samples are queued.
fn decode_some_frames(state: &Arc<Mutex<InternalState>>,
                      native_sample_rate: Option<f64>,
                      resample_state: &mut Option<ResampleState>) {
    let decode_ahead = prefs::get_decode_ahead();
    // briefly hold the lock to figure out how many frames are queued up
    let mut decoded = FRAME_QUEUE.lock().unwrap().iter()
        .fold(0.0, |total, el| total + ((el.data.len() - el.consumed)
                                      / (el.channel_count.max(1) as usize))
              as f64 / el.sample_rate as f64);
    // (We don't keep the queue locked during the inner loop, because we
    // want to hold up the audio callback as little as possible.)
    while decoded < decode_ahead {
        // TODO: end of playback
        let mut state = state.lock().unwrap();
        if state.future_song.is_none() { break }
        decoded += state.decode_some_frames(decode_ahead - decoded,
                                            native_sample_rate,
                                            resample_state);
    }
}

impl InternalState {
    /// If the "future stream" isn't open, tries to open it.
    fn check_stream(&mut self) -> anyhow::Result<()> {
        if self.future_stream.is_some() { return Ok(()) }
        if let Some(future_song) = self.future_song.as_ref() {
            self.future_stream = future_song.read().unwrap().open_stream();
            if let Some(ref mut stream) = self.future_stream {
                stream.find_stream_info()?;
                // TODO: don't panic!
                let best_stream = stream.find_best_stream()?;
                let durr = match best_stream {
                    Some(x) => stream.open_stream(x)?,
                    None => return Err(anyhow!("Is this not a music file?")),
                };
                future_song.set_duration(durr);
                Ok(())
            }
            else {
                return Err(anyhow!("Unable to open stream"))
            }
        }
        else {
            return Err(anyhow!("No song?"))
        }
    }
    /// Goes to the next song in the playlist, which might involve looping
    /// and/or reshuffling the playlist.
    fn next_song(&mut self) {
        let future_playlist = match self.future_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = future_playlist.maybe_refreshed();
        let playmode = playlist.get_playmode();
        let songs = playlist.get_songs();
        let cur_index = match self.future_song.as_ref() {
            Some(future_song) =>
                songs.iter().position(|x| x == future_song),
            None => None,
        };
        let next_index = match cur_index {
            None => Some(0),
            Some(x) if x == songs.len() - 1 => {
                if playmode == Playmode::End {
                    None
                }
                else {
                    if playlist.is_shuffled() {
                        // TODO: parkinglot lets us upgrade a read lock into
                        // a write lock
                        drop(playlist);
                        let mut playlist = self.future_playlist.as_ref()
                            .unwrap().write().unwrap();
                        playlist.resort(true);
                        self.future_song = playlist.get_songs().get(0)
                            .cloned();
                        self.future_stream = None;
                        return
                    }
                    else {
                        Some(0)
                    }
                }
            },
            Some(x) => Some(x + 1),
        };
        self.future_song = next_index.and_then(|x| songs.get(x)).cloned();
        self.future_stream = None;
    }
    /// Goes to the previous song in the playlist, or start the current one
    /// over if we're at the beginning of the shuffle *or* if this song is not
    /// in the active playlist.
    ///
    /// THIS IS NOT THE SAME BEHAVIOR AS THE `Prev` COMMAND!
    fn prev_song(&mut self) {
        let future_playlist = match self.future_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = future_playlist.maybe_refreshed();
        let songs = playlist.get_songs();
        let cur_index = match self.future_song.as_ref() {
            Some(future_song) => songs.iter().position(|x| x == future_song),
            None => None,
        };
        let next_index = match cur_index {
            None => None,
            Some(x) if x == 0 => None,
            Some(x) => Some(x - 1),
        };
        match next_index {
            None => (), // don't change future_song, just start stream over
            Some(x) => {
                self.future_song = songs.get(x).cloned();
            }
        };
        self.future_stream = None;
    }
    /// Figures out what to play next (if relevant), reshuffles playlist (if
    /// relevant), and decodes a few `AudioFrames`. Will stop after the given
    /// number of seconds of audio have been decoded, or if the song changes.
    /// Returns the number of seconds of audio decoded.
    pub fn decode_some_frames(&mut self, secs: f64,
                              native_sample_rate: Option<f64>,
                              resample_state: &mut Option<ResampleState>)
    -> f64 {
        if !self.future_song.is_none() {
            match self.check_stream() {
                Ok(_) => (),
                Err(x) => {
                    error!("While trying to open {:?}\n{:?}",
                           self.future_song.as_ref().unwrap(), x);
                    self.future_stream = None;
                    self.next_song();
                    return 0.0;
                }
            }
        }
        if !self.future_song.is_none() {
            let song_id = self.future_song.as_ref().unwrap().read().unwrap().get_id();
            if let Some(ref mut av) = self.future_stream {
                let mut decoded_so_far = 0.0;
                while decoded_so_far < secs && !self.future_song.is_none() {
                    let looping = self.future_playlist.as_ref().unwrap().read()
                        .unwrap().get_playmode() == Playmode::LoopOne;
                    let loop_spot: Option<f64> =
                        if looping {
                            self.future_song.as_ref()
                                .and_then(|x| x.read().unwrap().get_metadata()
                                          .get("loop_end").map(String::as_str)
                                          .and_then(|x| str::parse(x).ok()))
                        } else { None };
                    // true if we have encountered the loop spot
                    let mut endut = false;
                    let more_left = av.decode_some(|start_time, sample_rate, channel_count, mut data| {
                        if endut { return }
                        assert!(data.len() > 0);
                        assert!(channel_count > 0 && channel_count < 32);
                        if let Some(loop_spot) = loop_spot {
                            if loop_spot >= start_time {
                                let samples_in_frame =
                                    data.len() / channel_count as usize;
                                let duration = (samples_in_frame as f64)
                                    / sample_rate;
                                if loop_spot < start_time + duration {
                                    endut = true;
                                    let end_sample = (loop_spot - start_time)
                                        * sample_rate;
                                    let floored_end_sample =end_sample.floor();
                                    let end_sample =
                                        if end_sample == floored_end_sample {
                                            floored_end_sample - 1.0
                                        } else { floored_end_sample } as usize;
                                    let end_index = end_sample
                                        * channel_count as usize;
                                    assert!(end_index <= data.len());
                                    data.resize(end_index, 0.0);
                                }
                            }
                        }
                        decoded_so_far += (data.len() / channel_count as usize)
                            as f64 / sample_rate as f64;
                        let res =
                            resample_state.output(native_sample_rate,
                                                  AudioFrame {
                                                      song_id, consumed: 0,
                                                      time: start_time,
                                                      sample_rate: sample_rate,
                                                      channel_count, data,
                                                  });
                        match res {
                            Ok(_) => (),
                            Err(x) => error!("Error resampling audio: {}", x),
                        }
                    });
                    if endut {
                        let loop_spot: f64 =
                            self.future_song.as_ref()
                            .and_then(|x| x.read().unwrap().get_metadata()
                                      .get("loop_start").map(String::as_str)
                                      .and_then(|x| str::parse(x).ok()))
                            .unwrap_or(0.0);
                        av.seek_to_time(loop_spot);
                    }
                    else if !more_left {
                        if looping {
                            av.seek_to_time(0.0);
                        }
                        else {
                            self.next_song();
                            break
                        }
                    }
                }
                return decoded_so_far
            }
        }
        return 0.0
    }
    fn reset_to_heard_point(&mut self) -> anyhow::Result<()> {
        FRAME_QUEUE.lock().unwrap().clear();
        let (cur_song, timestamp) = self.active_song.as_ref().map(|(x,y)| (x.clone(), *y)).ok_or_else(|| anyhow!("Resetting to heard point but there's no heard song?"))?;
        if Some(&cur_song) != self.future_song.as_ref() {
            self.future_song = Some(cur_song);
            self.future_stream = None;
        }
        if self.check_stream().is_ok() {
            if let Some(stream) = self.future_stream.as_mut() {
                stream.seek_to_time(timestamp);
            }
        }
        Ok(())
    }
}

/// Returns whether mute is now active.
pub fn toggle_mute() -> bool {
    // TODO: reduce the lag time on the mute button
    let mut state = STATE.lock().unwrap();
    state.muted = !state.muted;
    state.muted
}

/// Set the mute state.
pub fn set_mute(nu: bool) {
    let mut state = STATE.lock().unwrap();
    state.muted = nu;
}

/// Given a specific user-visible "volume level", give the attenuation in
/// decibels.
pub fn volume_to_db(level: i32) -> f64 {
    let amplitude = (level as f64) / 100.0;
    let asq = amplitude * amplitude;
    asq.log10() * 10.0
}

