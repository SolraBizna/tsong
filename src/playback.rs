//! This module handles the current playback state; playing/paused, current
//! song, current playlist, etc. It also manages the actual playback device(s),
//! opening and closing and starting and stopping the stream.

use crate::*;

use std::{
    collections::VecDeque,
    sync::{Arc, RwLock, Mutex, atomic::Ordering},
    sync::mpsc::{Sender, Receiver, channel},
    time::Instant,
};

use portaudio::{
    stream::{Parameters, OutputSettings, OutputCallbackArgs},
    PortAudio,
    StreamCallbackResult,
};
use lazy_static::lazy_static;

/// The amount of data to try to keep in the audio queue. Totally arbitrary.
const SAMPLES_AHEAD: usize = 50000;
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
    Play(Option<LogicalSongRef>), Pause, Stop
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
    Playing, Paused, Stopped
}

impl Default for PlaybackStatus {
    fn default() -> PlaybackStatus { PlaybackStatus::Stopped }
}

#[derive(Default)]
pub struct InternalState {
    /// The song that the user is *currently hearing*.
    active_song: Option<LogicalSongRef>,
    /// The song that the user *will* be hearing if *all buffers currently
    /// queued are played*.
    future_song: Option<LogicalSongRef>,
    /// The playlist from which the *next* song will be drawn.
    future_playlist: Option<PlaylistRef>,
    /// The `GenerationValue` of `future_playlist` for which this shuffle is
    /// up to date.
    shuffle_generation: GenerationValue,
    /// The shuffled song list.
    shuffled_playlist: Vec<LogicalSongRef>,
    /// The playback thread will update this to reflect the current playback
    /// state, as it changes in response to commands.
    status: PlaybackStatus,
}

lazy_static! {
    static ref STATE: Arc<RwLock<InternalState>>
        = Arc::new(RwLock::new(Default::default()));
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
    let mut me = STATE.write().unwrap();
    if me.future_playlist != new_playlist {
        me.future_playlist = new_playlist;
        me.shuffle_generation.destroy();
        me.shuffled_playlist.clear();
    }
}

/// Request that the given song start playing from the beginning. This will
/// stop any playback currently in progress.
pub fn start_playing_song(new_song: LogicalSongRef) {
    send_playback_command(Stop);
    send_playback_command(Play(Some(new_song)));
}

pub fn send_playback_command(wat: PlaybackCommand) {
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
            rem.copy_from_slice(&next_data[..rem.len()]);
            now += (rem.len() / channel_count as usize) as f64 / sample_rate;
            next_el.consumed += rem.len();
            rem = &mut [];
        }
        else {
            (&mut rem[..next_data.len()]).copy_from_slice(next_data);
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
            // We use `try_read()` instead of `read()` to lock the state
            // because we still hold the queue lock; since the playback thread
            // will acquire the queue lock while holding the state lock, if we
            // try to do the reverse we could end up with deadlock.
            // If we couldn't get the lock, assume that playback is ongoing.
            // We'll play some extra silence, but that's okay.
            // TODO: "predicted playback status" is what matters here, not
            // the user-visible status
            let status = STATE.try_read().map(|x| x.status)
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

/// DELETE ME
fn make_square_vec() -> Vec<f32> {
    let mut ret = Vec::with_capacity(50000);
    for i in (0..100).rev() {
        for _ in 0..250 {
            ret.push(0.1 * (i as f32 / 100.0));
        }
        for _ in 0..250 {
            ret.push(-0.1 * (i as f32 / 100.0));
        }
    }
    ret
}

/// Figures out what to play next (if relevant), reshuffles playlist (if
/// relevant), and decodes as many `AudioFrame`s as necessary to make sure we
/// have SAMPLES_AHEAD (times number of channels) samples queued.
fn decode_some_frames(state: &Arc<RwLock<InternalState>>) {
    // briefly hold the lock to figure out how many frames are queued up
    let mut sample_count = FRAME_QUEUE.lock().unwrap().iter()
        .fold(0, |total, el| total + (el.data.len() - el.consumed)
              / (el.channel_count.max(1) as usize));
    let square_vec = make_square_vec();
    while sample_count < SAMPLES_AHEAD {
        // We don't keep the queue locked during the inner loop, because we
        // want to step on the audio callback as little as possible.
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(1),
            time: 0.0,
            sample_rate: 44100.0,
            channel_count: 1,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(2),
            time: 10.0,
            sample_rate: 48000.0,
            channel_count: 1,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(3),
            time: 100.0,
            sample_rate: 44100.0,
            channel_count: 2,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(4),
            time: 1000.0,
            sample_rate: 48000.0,
            channel_count: 2,
            data: square_vec.clone(),
            consumed: 0,
        });
        sample_count += square_vec.len() * 3;
    }
}

fn playback_thread(state: Arc<RwLock<InternalState>>,
                   playback_control_rx: Receiver<PlaybackThreadMessage> ) {
    let pa = PortAudio::new().expect("Could not initialize PortAudio");
    loop {
        debug_assert_eq!(state.read().unwrap().status,
                         PlaybackStatus::Stopped);
        // Not playing
        let requested_song = match playback_control_rx.recv() {
            Err(_) => return, // bye bye...
            Ok(PlaybackThreadMessage::Command(Play(song)))
                => song,
            Ok(_) => continue, // still not playing!
        };
        {
            let mut state = state.write().unwrap();
            state.status = PlaybackStatus::Playing;
            state.active_song = requested_song;
        }
        while state.read().unwrap().status != PlaybackStatus::Stopped {
            let (sample_rate, channel_count) = {
                decode_some_frames(&state);
                // double lock in the common case :(
                match FRAME_QUEUE.lock().unwrap().get(0) {
                    None => {
                        state.write().unwrap().status
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
            'alive_loop: loop {
                let mut got_message = false;
                while let Some(message) =
                        if got_message { playback_control_rx.try_recv().ok() }
                        else { playback_control_rx.recv().ok() } {
                    got_message = true;
                    match message {
                        PlaybackThreadMessage::CallbackRan => (),
                        // TODO: pause, play
                        PlaybackThreadMessage::Command(Stop) => {
                            let mut state = state.write().unwrap();
                            state.status = PlaybackStatus::Stopped;
                            state.active_song = None;
                            let _ = stream.abort();
                            break 'alive_loop;
                        },
                        _ => continue,
                    }
                }
                // Now run any necessary periodic tasks.
                let now = if BROKEN_STREAM_TIME.load(Ordering::Acquire) {
                    BROKEN_EPOCH.elapsed().as_secs_f64()
                } else { stream.time() };
                //eprintln!("Now: {}", now);
                { 
                    let mut report_queue = REPORT_QUEUE.lock().unwrap();
                    while report_queue.get(0).map(|x| x.0 <= now)
                            .unwrap_or(false) {
                        //eprintln!("Report: {:?}", report_queue.get(0));
                        let (_time, el) = report_queue.pop_front().unwrap();
                        match el {
                            SongPlaying { song_id, time: _songtime } => {
                                let mut state = state.write().unwrap();
                                let change_song = match &state.active_song {
                                    &Some(ref x) => x.read().unwrap().get_id()
                                        != song_id,
                                    &None => true
                                };
                                if change_song {
                                    state.active_song = Some(logical::get_song_by_song_id(song_id).expect("Playback changed to a song not in the database!"));
                                }
                            },
                            SampleFormatChanged => {
                                let _ = stream.abort();
                                break 'alive_loop;
                            },
                            PlaybackFinished => {
                                let _ = stream.abort();
                                state.write().unwrap().status = PlaybackStatus::Stopped;
                                break 'alive_loop;
                            },
                        }
                    }
                }
                decode_some_frames(&state);
            }
            REPORT_QUEUE.lock().unwrap().clear();
        }
    }
}
