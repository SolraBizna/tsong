//! This module handles the current playback state; playing/paused, current
//! song, current playlist, etc. It also manages the actual playback device(s),
//! opening and closing and starting and stopping the stream.

use crate::*;

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

/// The amount of data to try to keep in the audio queue. Totally arbitrary.
const SAMPLES_AHEAD: usize = 100000;
/// The number of buffers to keep around. (This applies to the audio API,
/// unlike `SAMPLES_AHEAD`, which applies only to our own queue.)
const DESIRED_LATENCY: f64 = 1.0;

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
        Err(_) => eprintln!("WARNING: Playback thread is dead"),
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
            eprintln!("Stream time is broken on this driver!");
            true_now // don't add latency, we're hopefully priming buffers
        }
        else {
            true_now + DESIRED_LATENCY
        }
    }
    else {
        time.buffer_dac
    };
    let volume = prefs::get_volume() as f32 / 100.0;
    let volume = volume * volume;
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
            // TODO: "predicted playback status" is what matters here, not
            // the user-visible status
            let status = STATE.try_lock().map(|x| x.status)
                .unwrap_or(PlaybackStatus::Playing);
            if status != PlaybackStatus::Playing  {
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
                   playback_control_rx: Receiver<PlaybackThreadMessage> ) {
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
            // - We hit the end of the playlist and looping isn't enabled.
            //   Finish up. (This might be handled elsewhere?)
            let (sample_rate, channel_count) = {
                decode_some_frames(&state);
                // double lock in the common case :(
                match FRAME_QUEUE.lock().unwrap().get(0) {
                    None => {
                        state.lock().unwrap().status
                            = PlaybackStatus::Stopped;
                        break;
                    },
                    Some(ref x) =>
                        (x.sample_rate, x.channel_count),
                }
            };
            *CURRENT_AUDIO_FORMAT.lock().unwrap()
                = (sample_rate, channel_count);
            // Time to open a new stream...
            let parameters = Parameters::new(pa.default_output_device()
                                             .expect("No default output \
                                                      device?"),
                                             channel_count,
                                             true, // interleaved
                                             DESIRED_LATENCY);
            let flags = portaudio::stream_flags
                ::PA_PRIME_OUTPUT_BUFFERS_USING_STREAM_CALLBACK;
            let settings = OutputSettings::with_flags(parameters, sample_rate,
                                                      0, flags);
            let mut stream = pa.open_non_blocking_stream(settings,
                                                         playback_callback)
                .expect("Unable to open audio stream"); // TODO: fail better
            // just in case...
            REPORT_QUEUE.lock().unwrap().clear();
            BROKEN_STREAM_TIME.store(false, Ordering::Relaxed);
            decode_some_frames(&state);
            stream.start()
                .expect("Unable to start audio stream");
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
                                state.active_song = Some((logical::get_song_by_song_id(song_id).expect("Playback changed to a song not in the database!"), songtime));
                            }
                            else {
                                state.active_song.as_mut().unwrap().1 = songtime;
                            }
                        },
                        SampleFormatChanged => {
                            break 'alive_loop;
                        },
                        PlaybackFinished => {
                            state.lock().unwrap().status = PlaybackStatus::Stopped;
                            break 'alive_loop;
                        },
                    }
                }
                // release the lock...
                drop(report_queue);
                // ...so that we're not holding it during the (expensive)
                // decoding step
                decode_some_frames(&state);
            }
            // Clean up!
            let _ = stream.abort();
            // Any reports after we decided to kill the stream are of no
            // consequence.
            REPORT_QUEUE.lock().unwrap().clear();
            // Any frames that the user was *going* to hear after they hit the
            // end of playback are of no consequence.
            FRAME_QUEUE.lock().unwrap().clear();
            let mut state = state.lock().unwrap();
            match state.status {
                PlaybackStatus::Playing => (),
                PlaybackStatus::Paused => {
                    // Whatever song the user was hearing when they hit pause,
                    // that's where we paused.
                    let (cur_song, timestamp) = state.active_song.as_ref().map(|(x,y)| (x.clone(), *y)).expect("Paused, but there is no current song?");
                    if Some(&cur_song) != state.future_song.as_ref() {
                        state.future_song = Some(cur_song);
                        state.future_stream = None;
                    }
                    if state.check_stream().is_ok() {
                        if let Some(stream) = state.future_stream.as_mut() {
                            stream.seek_to_time(timestamp);
                        }
                    }
                },
                PlaybackStatus::Stopped => {
                    // There is no longer an active song.
                    state.future_song = None;
                    state.active_song = None;
                },
            }
        }
    }
}

/// Wrapper that repeatedly calls `state.decode_some_frames()` until enough
/// samples are queued.
fn decode_some_frames(state: &Arc<Mutex<InternalState>>) {
    // briefly hold the lock to figure out how many frames are queued up
    let mut sample_count = FRAME_QUEUE.lock().unwrap().iter()
        .fold(0, |total, el| total + (el.data.len() - el.consumed)
              / (el.channel_count.max(1) as usize));
    // (We don't keep the queue locked during the inner loop, because we
    // want to hold up the audio callback as little as possible.)
    while sample_count < SAMPLES_AHEAD {
        // TODO: end of playback
        let mut state = state.lock().unwrap();
        sample_count += state.decode_some_frames(SAMPLES_AHEAD - sample_count);
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
        let playlist = self.future_playlist.as_ref().unwrap()
            .maybe_refreshed();
        let songs = playlist.get_songs();
        let cur_index = match self.future_song.as_ref() {
            Some(future_song) =>
                songs.iter().position(|x| x == future_song),
            None => None,
        };
        let next_index = match cur_index {
            None => 0,
            Some(x) if x == songs.len() - 1 => {
                // TODO: reshuffle, loop flag
                0
            },
            Some(x) => x + 1,
        };
        self.future_song = songs.get(next_index).cloned();
        self.future_stream = None;
    }
    /// Goes to the previous song in the playlist, or start the current one
    /// over if we're at the beginning of the shuffle *or* if this song is not
    /// in the active playlist.
    ///
    /// THIS IS NOT THE SAME BEHAVIOR AS THE `Prev` COMMAND!
    fn prev_song(&mut self) {
        let playlist = self.future_playlist.as_ref().unwrap()
            .maybe_refreshed();
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
    /// number of samples-per-channel have been decoded, or if the song
    /// changes. Returns the number of samples decoded.
    pub fn decode_some_frames(&mut self, sample_count: usize) -> usize {
        if !self.future_song.is_none() {
            match self.check_stream() {
                Ok(_) => (),
                Err(x) => {
                    eprintln!("Error while trying to open {:?}\n{:?}",
                              self.future_song.as_ref().unwrap(), x);
                    self.future_stream = None;
                    self.next_song();
                    return 0;
                }
            }
        }
        if !self.future_song.is_none() {
            let song_id = self.future_song.as_ref().unwrap().read().unwrap().get_id();
            if let Some(ref mut av) = self.future_stream {
                let mut decoded_so_far = 0;
                while decoded_so_far < sample_count {
                     let more_left = av.decode_some(|time, sample_rate, channel_count, data| {
                        assert!(data.len() > 0);
                        assert!(channel_count > 0 && channel_count < 32);
                        decoded_so_far += data.len() / channel_count as usize;
                        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
                            song_id, consumed: 0,
                            time, sample_rate, channel_count, data,
                        });
                    });
                    if !more_left {
                        self.next_song();
                        break
                    }
                }
                return decoded_so_far
            }
        }
        return 0
    }
}

