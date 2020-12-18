//! This module handles the current playback state; playing/paused, current
//! song, current playlist, etc. It also manages the actual playback device(s),
//! opening and closing and starting and stopping the stream.

use crate::*;

use std::{
    collections::VecDeque,
    sync::{Arc, RwLock, Mutex},
    sync::mpsc::{Sender, Receiver, channel},
};

use portaudio::{
    stream::{Parameters, OutputSettings, OutputCallbackArgs},
    PortAudio,
    StreamCallbackResult,
};
use lazy_static::lazy_static;

/// The amount of data to try to keep in the audio queue. Totally arbitrary.
const SAMPLES_AHEAD: usize = 50000;

struct AudioFrame {
    song_id: SongID,
    sample_rate: f64,
    channel_count: i32,
    data: Vec<f32>, // hooray! lots of copying!
    consumed: usize, // number of indices within data that have been consumed
}

#[derive(Debug)]
pub enum PlaybackCommand {
    Play(Option<LogicalSongRef>), Pause, Stop
}
use PlaybackCommand::*;

#[derive(Debug)]
enum CallbackReport {
    /// The callback processed some audio from the queue, uneventfully.
    Continuing,
    /// The callback ran out of queue data, but believes that playback is
    /// still in progress, and so is continuing to fill with zeroes.
    QueueUnderrun,
    /// The next data in the queue is at a different sample rate. The stream
    /// has been terminated.
    SampleFormatChanged((f64, i32)),
    /// The queue is empty and is expected to stay empty. The stream has been
    /// terminated.
    Finished
}
use CallbackReport::*;

#[derive(Debug)]
enum PlaybackThreadMessage {
    Command(PlaybackCommand),
    Report(CallbackReport),
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
    /// The song that is *currently being played* by the playback thread.
    active_song: Option<LogicalSongRef>,
    /// The playlist from which the *next* song will be drawn.
    active_playlist: Option<PlaylistRef>,
    /// The `GenerationValue` of `active_playlist` for which this shuffle is
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
    static ref CURRENT_AUDIO_FORMAT: Mutex<(f64, i32)>
        = Mutex::new(Default::default());
}

/// Selects a different playlist to be active, without changing the active
/// song.
pub fn set_active_playlist(new_playlist: Option<PlaylistRef>) {
    let mut me = STATE.write().unwrap();
    if me.active_playlist != new_playlist {
        me.active_playlist = new_playlist;
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

fn send_callback_report(wat: CallbackReport) {
    match PLAYBACK_CONTROL_TX.lock().unwrap().as_ref().unwrap().send(PlaybackThreadMessage::Report(wat)) {
        Ok(_) => (),
        Err(_) => eprintln!("WARNING: Playback thread is dead (seen by callback)"),
    }
}

fn playback_callback(args: OutputCallbackArgs<f32>) -> StreamCallbackResult {
    // destructure parameters
    let OutputCallbackArgs {
        buffer,
        ..
    } = args;
    let mut rem = buffer;
    let mut queue = FRAME_QUEUE.lock().unwrap();
    let current_audio_format = *CURRENT_AUDIO_FORMAT.lock().unwrap();
    while rem.len() > 0 {
        let next_el = match queue.get_mut(0) {
            None => break,
            Some(el) => el,
        };
        if (next_el.sample_rate, next_el.channel_count)!=current_audio_format {
            break
        }
        let next_data = &next_el.data[next_el.consumed..];
        if next_data.len() > rem.len() {
            rem.copy_from_slice(&next_data[..rem.len()]);
            next_el.consumed += rem.len();
            rem = &mut [];
        }
        else {
            (&mut rem[..next_data.len()]).copy_from_slice(next_data);
            rem = &mut rem[next_data.len()..];
            queue.pop_front();
        }
    }
    // fill rest with zeroes
    // (slice::fill isn't stable yet)
    for el in rem.iter_mut() { *el = 0.0; }
    // so. why did we stop?
    let (report, result) = match queue.get(0) {
        None => {
            // No audio was left in the queue. We might have had a queue
            // underrun, or we might have reached the end of playback.
            // We use `try_read()` instead of `read()` to lock the state
            // because we still hold the queue lock; since the playback thread
            // will acquire the queue lock while holding the state lock, if we
            // try to do the reverse we could end up with deadlock.
            // If we couldn't get the lock, assume that playback is ongoing.
            // This might result in a false underrun report.
            // TODO: "predicted playback status" is what matters here, not
            // the user-visible status
            let status = STATE.try_read().map(|x| x.status)
                .unwrap_or(PlaybackStatus::Playing);
            match status {
                PlaybackStatus::Playing => {
                    if rem.len() == 0 {
                        (Continuing, StreamCallbackResult::Continue)
                    }
                    else {
                        (QueueUnderrun, StreamCallbackResult::Continue)
                    }
                },
                _ => (Finished, StreamCallbackResult::Complete)
            }
        },
        Some(x) => {
            if (x.sample_rate, x.channel_count) != current_audio_format {
                (SampleFormatChanged((x.sample_rate, x.channel_count)), StreamCallbackResult::Continue)
            }
            else {
                (Continuing, StreamCallbackResult::Continue)
            }
        },
    };
    send_callback_report(report);
    result
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
    let mut sample_count = FRAME_QUEUE.lock().unwrap().iter()
        .fold(0, |total, el| total + (el.data.len() - el.consumed)
              / (el.channel_count.max(1) as usize));
    let square_vec = make_square_vec();
    while sample_count < SAMPLES_AHEAD {
        // We don't keep the queue locked during the inner loop, because we
        // want to step on the audio callback as little as possible.
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(42),
            sample_rate: 44100.0,
            channel_count: 1,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(43),
            sample_rate: 48000.0,
            channel_count: 1,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(44),
            sample_rate: 44100.0,
            channel_count: 2,
            data: square_vec.clone(),
            consumed: 0,
        });
        FRAME_QUEUE.lock().unwrap().push_back(AudioFrame {
            song_id: SongID::from_db(45),
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
                                             0.5); // suggested latency
            let flags = portaudio::stream_flags
                ::PA_PRIME_OUTPUT_BUFFERS_USING_STREAM_CALLBACK;
            let settings = OutputSettings::with_flags(parameters, sample_rate,
                                                      0, flags);
            let mut stream = pa.open_non_blocking_stream(settings,
                                                         playback_callback)
                .expect("Unable to open audio stream"); // TODO: fail better
            stream.start()
                .expect("Unable to start audio stream");
            loop {
                match playback_control_rx.recv() {
                    Err(_) => return, // bye bye...
                    Ok(PlaybackThreadMessage::Report(Continuing)) => {
                        decode_some_frames(&state);
                    },
                    Ok(PlaybackThreadMessage::Report(QueueUnderrun)) => {
                        eprintln!("Audio underrun!"); // TODO report better
                        decode_some_frames(&state);
                    },
                    Ok(PlaybackThreadMessage::Report(SampleFormatChanged(nu))) => {
                        if *CURRENT_AUDIO_FORMAT.lock().unwrap() != nu {
                            stream.stop();
                            break; // we need to recreate the stream
                        }
                        // we'll get some extra sample rate change requests;
                        // when we do, ignore them
                    },
                    Ok(PlaybackThreadMessage::Report(Finished)) => {
                        stream.stop();
                        break; // we will be going quiescent, or maybe making
                        // a new stream, if the user requested more playback
                    },
                    // TODO: pause, play
                    Ok(PlaybackThreadMessage::Command(Stop)) => {
                        let mut state = state.write().unwrap();
                        state.status = PlaybackStatus::Stopped;
                        state.active_song = None;
                        stream.abort();
                        FRAME_QUEUE.lock().unwrap().clear();
                        break;
                    },
                    Ok(_) => continue,
                }
            }
        }
    }
}
