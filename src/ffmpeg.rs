//! Safe bindings to the subset of the ffmpeg-dev crate that we use.

use log::{trace, error, debug, info, warn};
use anyhow::anyhow;
use ffmpeg_dev::sys as ff;
use ffmpeg_dev::extra::defs as ffdefs;
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString},
    path::Path,
    ptr::null_mut,
    mem::transmute,
};

/// Turn an FFMPEG error code into an error string.
fn ffres_to_string(code: libc::c_int) -> String {
    const BUF_SIZE: usize = 1024;
    let mut buf: [libc::c_char; BUF_SIZE] = [0; BUF_SIZE];
    let cstr = unsafe {
        // Why not BUF_SIZE? Well, because I'm not 100% sure that av_strerror
        // always null terminates the buffer, that's why...
        ff::av_strerror(code, &mut buf[0], BUF_SIZE - 1);
        // see above... though if it wrote past the end of the buffer we're
        // kinda screwed anyway
        buf[BUF_SIZE-1] = 0;
        CStr::from_ptr(&buf[0])
    };
    cstr.to_string_lossy().into_owned()
}

/// Wrap a return value from an FFMPEG function that returns 0 on success and
/// fails if it returns a value **NOT EQUAL** to 0.
fn fferr_ne(code: libc::c_int) -> anyhow::Result<()> {
    match code {
        0 => Ok(()),
        x => Err(anyhow!("{}", ffres_to_string(x))),
    }
}

/// Wrap a return value from an FFMPEG function that returns 0 on success and
/// fails if it returns a value **LESS THAN** 0.
fn fferr_lt(code: libc::c_int) -> anyhow::Result<libc::c_int> {
    match code {
        x if x >= 0 => Ok(x.into()),
        x => Err(anyhow!("{}", ffres_to_string(x))),
    }
}

/// Transcribes the contents of an AVDictionary onto the given BTreeMap.
fn transcribe_dict(out: &mut BTreeMap<String, String>,
                   dict: *mut ff::AVDictionary) {
    if dict.is_null() { return }
    let mut tag: *mut ff::AVDictionaryEntry = null_mut();
    let empty_cstring = CString::new("").unwrap();
    loop {
        tag = unsafe {
            ff::av_dict_get(dict, empty_cstring.as_ptr(), tag,
                            ff::AV_DICT_IGNORE_SUFFIX as libc::c_int)
        };
        let tag = match unsafe { tag.as_ref() } {
            Some(x) => x,
            None => break, // null = no more to go
        };
        let key = unsafe { CStr::from_ptr(tag.key) }.to_string_lossy();
        let value = unsafe { CStr::from_ptr(tag.value) }.to_string_lossy();
        out.insert(key.into_owned(), value.into_owned());
    }
}

/// Converts an input timestamp in seconds-from-beginning to an output
/// timestamp in stream-specific units.
fn float_time_to_fftime(ftime: f64, inner: &ff::AVFormatContext,
                               stream: &ff::AVStream) -> i64 {
    let timebase = &stream.time_base;
    let start_pts = match stream.start_time {
        x if x == unsafe { ffdefs::av_nopts_value() } => match inner.start_time {
            x if x == unsafe { ffdefs::av_nopts_value() } => 0,
            x => x,
        },
        x => x,
    };
    ((ftime * timebase.den as f64) / (timebase.num as f64)).floor() as i64
        + start_pts
}
// TODO: fftime_to_float_time

/// Wraps an (input!) `AVFormatContext`
pub struct AVFormat {
    /// A pointer to the `AVFormatContext` that we're managing, or null if
    /// we've been closed.
    inner: *mut ff::AVFormatContext,
    /// A pointer to the `AVCodecContext` we're managing, or null if we haven't
    /// opened the file for decoding or if we've been closed.
    codec_ctx: *mut ff::AVCodecContext,
    /// The last stream opened. Valid if codec_ctx is non-null;
    stream: libc::c_int,
    /// A single packet of encoded data from the muxer.
    packet: ff::AVPacket,
    /// A single frame—or several frames, for PCM/etc—of decoded data from the
    /// codec.
    frame: *mut ff::AVFrame,
    /// Some half-consumed data that we decoded after a seek. We consume some
    /// frames, and then possibly part of a frame, which means there may be
    /// a partial frame and then some complete frames left over.
    leftovers: Vec<(f64, f64, i32, Vec<f32>)>,
}

/// This can be sent, as long as it's `Sync`ed...
unsafe impl Send for AVFormat {}

impl AVFormat {
    fn get_stream_ref(&self, stream: libc::c_int) -> &ff::AVStream {
        let inner = unsafe { self.inner.as_ref() }.unwrap();
        if stream < 0 || (stream as u32) > inner.nb_streams {
            panic!("logic error: invalid stream index: {}", stream);
        }
        unsafe {
            inner.streams.offset(stream as isize).read().as_ref()
                .expect("null stream?!")
        }
    }
    fn maybe_close_codec(&mut self) {
        if !self.codec_ctx.is_null() {
            unsafe {
                ff::avcodec_free_context(&mut self.codec_ctx);
            }
            self.codec_ctx = null_mut();
        }
        if !self.frame.is_null() {
            unsafe {
                ff::av_frame_free(&mut self.frame)
            }
        }
    }
    /// Calls `avformat_open_input` for the given path.
    pub fn open_input(path: &Path) -> anyhow::Result<AVFormat> {
        let path_str = match path.to_str() {
            Some(x) => x,
            None => return Err(anyhow!("Path contains invalid UTF-8")),
        };
        let path_cstring = CString::new(path_str)
            .expect("Internal error: Unable to convert path into C string?");
        let mut inner: *mut ff::AVFormatContext = null_mut();
        fferr_ne(unsafe { ff::avformat_open_input(&mut inner,
                                                  path_cstring.as_ptr(),
                                                  null_mut(),
                                                  null_mut())})?;
        assert!(!inner.is_null());
        Ok(AVFormat { inner, codec_ctx: null_mut(), stream: -1,
                      frame: null_mut(),
                      packet: unsafe { std::mem::zeroed() },
                      leftovers: Vec::new() })
    }
    /// Calls `avformat_find_stream_info`.
    pub fn find_stream_info(&mut self) -> anyhow::Result<()> {
        assert!(!self.inner.is_null());
        fferr_ne(unsafe { ff::avformat_find_stream_info(self.inner,
                                                        null_mut())
        })
    }
    /// Calls `av_find_best_stream`, to find the best audio stream.
    /// Returns `Ok(Some(n))` if `n` is the best audio stream, `Ok(None)` if
    /// it's not a music file at all, and `Err(...)` if any other error occurs.
    ///
    /// Make sure to call `find_stream_info` first.
    pub fn find_best_stream(&mut self) -> anyhow::Result<Option<libc::c_int>> {
        assert!(!self.inner.is_null());
        match unsafe { ff::av_find_best_stream(self.inner,
                                            ff::AVMediaType_AVMEDIA_TYPE_AUDIO,
                                               -1, -1, null_mut(),
                                               0) } {
            x if x >= 0 => Ok(Some(x)),
            x if x == unsafe { ffdefs::averror_stream_not_found() }
                => Ok(None),
            /* TODO: There's got to be a better way */
            x => fferr_lt(x).map(|x| Some(x) /* not reached */),
        }
    }
    /// Reads the metadata for the file, and for the given stream. Returns it
    /// in aggregate.
    pub fn read_metadata(&mut self, stream: Option<libc::c_int>)
    -> BTreeMap<String, String> {
        let inner = unsafe { self.inner.as_ref() }.unwrap();
        let mut ret = BTreeMap::new();
        transcribe_dict(&mut ret, inner.metadata);
        if let Some(stream) = stream {
            let stream_ref = self.get_stream_ref(stream);
            transcribe_dict(&mut ret, stream_ref.metadata);
        }
        ret
    }
    /// Estimates the duration of the given stream, in seconds.
    pub fn estimate_duration(&mut self, stream: libc::c_int) -> u32 {
        let inner = unsafe { self.inner.as_ref() }.unwrap();
        if stream >= 0 && (stream as u32) < inner.nb_streams {
            let stream_ref = unsafe {
                inner.streams.offset(stream as isize).read().as_ref()
                    .unwrap()
            };
            // TODO: if no stream duration, use the global duration and
            // `AV_TIME_BASE`?
            let num = stream_ref.duration.saturating_mul(stream_ref.time_base.num as i64);
            let den = stream_ref.time_base.den as i64;
            ((num + den / 2) / den)
                .min(u32::MAX as i64).max(0) as u32
        }
        else { panic!("logic error: invalid stream index: {}", stream); }
    }
    /// Calls `avformat_close_input`. This gets called automatically if this
    /// context is dropped without being closed.
    pub fn close_input(&mut self) {
        self.maybe_close_codec();
        if !self.inner.is_null() {
            unsafe {
                ff::avformat_close_input(&mut self.inner);
            }
            self.inner = null_mut();
        }
    }
    /// Opens the given audio stream for playback. Returns the estimated
    /// duration of the opened stream.
    pub fn open_stream(&mut self, stream: libc::c_int) -> anyhow::Result<u32> {
        self.maybe_close_codec();
        let durr = self.estimate_duration(stream);
        let stream_ref = self.get_stream_ref(stream);
        let codecpar = unsafe { stream_ref.codecpar.as_ref().unwrap() };
        let decoder = unsafe {
            ff::avcodec_find_decoder(codecpar.codec_id).as_ref()
        }.ok_or_else(|| anyhow!("opening stream {}, couldn't find decoder",
                                stream))?;
        unsafe {
            /* Somebody told me you couldn't reuse the codec context in the
             * stream struct. It looks like they were wrong, but here we are.
             * Being in a cargo cult feels weird. */
            let mut nu_ctx = ff::avcodec_alloc_context3(decoder);
            match ff::avcodec_parameters_to_context(nu_ctx, codecpar) {
                0 => (),
                x => {
                    ff::avcodec_free_context(&mut nu_ctx);
                    Err(anyhow!("opening stream {}, ffmpeg error {}",
                                stream, x))?
                },
            }
            self.codec_ctx = nu_ctx;
            self.stream = stream;
            ff::av_init_packet(&mut self.packet);
            match ff::avcodec_open2(self.codec_ctx, decoder, null_mut()) {
                0 => (),
                x => {
                    ff::avcodec_free_context(&mut nu_ctx);
                    Err(anyhow!("opening stream {}, ffmpeg error {}",
                                stream, x))?
                },
            }
        }
        Ok(durr)
    }
    fn decode_from_packet<H>(&mut self, packet: &ff::AVPacket, handler: &mut H)
    -> anyhow::Result<i32>
    where H: FnMut(f64, f64, i32, Vec<f32>) {
        let mut got_frame: libc::c_int = 0;
        trace!("DECODE!");
        trace!("Packet: {:?} ... {:?}", self.packet.data, self.packet.size);
        let len = fferr_lt(unsafe {
            ff::avcodec_decode_audio4(self.codec_ctx, self.frame,
                                      &mut got_frame, packet)
        })?;
        trace!("DECODED!");
        if got_frame != 0 {
            let frame = unsafe { self.frame.as_ref().unwrap() };
            let inner = unsafe { self.inner.as_ref().unwrap() };
            let stream_ref = self.get_stream_ref(self.stream);
            trace!("{}, {}", frame.pts, inner.start_time);
            let time = frame.pts //(frame.pts - inner.start_time)
                .saturating_mul(stream_ref.time_base.num as i64) as f64
                / (stream_ref.time_base.den as f64);
            let sample_rate = frame.sample_rate as f64;
            let channel_count = frame.channels as i32;
            // TODO: recycle buffers
            let mut buf = Vec::new();
            match frame.format {
                ff::AVSampleFormat_AV_SAMPLE_FMT_U8 =>
                    expand_packed_audio::<u8>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_U8P =>
                    expand_planar_audio::<u8>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S16 =>
                    expand_packed_audio::<i16>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S16P =>
                    expand_planar_audio::<i16>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S32 =>
                    expand_packed_audio::<i32>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S32P =>
                    expand_planar_audio::<i32>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S64 =>
                    expand_packed_audio::<i64>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_S64P =>
                    expand_planar_audio::<i64>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_FLT =>
                    expand_float_packed_audio(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_FLTP =>
                    expand_float_planar_audio(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_DBL =>
                    expand_packed_audio::<f64>(frame, &mut buf),
                ff::AVSampleFormat_AV_SAMPLE_FMT_DBLP =>
                    expand_planar_audio::<f64>(frame, &mut buf),
                x => {
                    return Err(anyhow!("Unknown AVSampleFormat: {}", x))
                }
            }
            handler(time, sample_rate, channel_count, buf);
        }
        Ok(len)
    }
    /// Decodes some audio from the current playback position, and advances
    /// the playback position.
    ///
    /// Returns true if there is still more data in the file, or false if it
    /// has concluded.
    ///
    /// Handler parameters:
    /// 
    /// - `time`: The time in seconds from the beginning of the song that this
    ///   decoded data starts at.
    /// - `sample_rate`: The sample rate of this decoded data. In some formats,
    ///   this can change mid-stream.
    /// - `channel_count`: The channel count of this decoded data. 1 = mono,
    ///   2 = stereo, etc. In some formats, this can change mid-stream.
    /// - `buf`: Buffer containing packed float audio data.
    ///
    /// If there are errors in decoding, playback will stop and the error will
    /// go into a log somewhere.
    pub fn decode_some<H>(&mut self, mut handler: H)
        -> bool
    where H: FnMut(f64, f64, i32, Vec<f32>)
    {
        assert!(!self.inner.is_null());
        assert!(!self.codec_ctx.is_null());
        let mut leftovers = Vec::new();
        std::mem::swap(&mut leftovers, &mut self.leftovers);
        for p in leftovers.into_iter() {
            handler(p.0, p.1, p.2, p.3);
        }
        self.packet.data = null_mut();
        self.packet.size = 0;
        loop {
            match unsafe { ff::av_read_frame(self.inner, &mut self.packet)} {
                0 => (),
                x => {
                    if x == unsafe { ffdefs::averror_eof() } {
                        // End of file. Maybe put out a bit of buffered data?
                        let packet = self.packet;
                        match self.decode_from_packet(&packet, &mut handler) {
                            Ok(_) => (),
                            Err(x) =>
                                error!("While decoding audio: {:?}", x),
                        };
                    }
                    else {
                        error!("av_read_frame: {}", x);
                    }
                    return false
                },
            }
            if self.packet.stream_index != self.stream {
                unsafe { ff::av_free_packet(&mut self.packet) }
                continue
            }
            else { break }
        }
        if self.frame.is_null() {
            unsafe {
                self.frame = ff::av_frame_alloc();
                if self.frame.is_null() {
                    error!("av_frame_alloc failed");
                    return false
                }
            }
        }
        let mut packet = self.packet;
        while packet.size > 0 {
            let len = match self.decode_from_packet(&packet, &mut handler) {
                Ok(x) => x,
                Err(x) => {
                    error!("While decoding audio: {:?}", x);
                    return false
                },
            };
            packet.data = unsafe { packet.data.offset(len as isize) };
            packet.size = packet.size - len;
        }
        unsafe { ff::av_free_packet(&mut self.packet) }
        true
    }
    /// Seek to the given time in the open stream. This may entail some
    /// decoding. Tries to be as exact as possible.
    ///
    /// If there are errors, they'll go into a log somewhere...
    pub fn seek_to_time(&mut self, target: f64) {
        let inner = unsafe { self.inner.as_ref() }.unwrap();
        assert!(!self.codec_ctx.is_null());
        let stream_ref = self.get_stream_ref(self.stream);
        let target_timestamp
            = float_time_to_fftime(target, inner, stream_ref);
        match unsafe { ff::av_seek_frame(self.inner, self.stream,
                                         target_timestamp,
                                         ff::AVSEEK_FLAG_BACKWARD as i32)} {
            0 => (),
            x => {
                error!("av_seek_frame returned {}", x);
                return; // well, we tried
            },
        }
        unsafe { ff::avcodec_flush_buffers(self.codec_ctx) };
        debug!("Seeking to {} = {}!", target, target_timestamp);
        self.leftovers.clear();
        let mut leftovers = Vec::new();
        // repeat until we start getting data or we run out of data
        while leftovers.len() == 0 &&
            self.decode_some(|start_time, sample_rate, channel_count, mut buf|{
                let end_time = start_time + (buf.len() / channel_count
                                             as usize) as f64 / sample_rate;
                if end_time < target {
                    // do nothing
                }
                else if start_time >= target {
                    // pure leftover!
                    leftovers.push((start_time, sample_rate,
                                    channel_count, buf));
                }
                else {
                    let cutoff_index = ((target - start_time) * sample_rate)
                        .ceil() as usize * channel_count as usize;
                    buf.drain(..cutoff_index);
                    leftovers.push((target, sample_rate, channel_count, buf));
                }
            }) {}
        // Do a very fast fade-in over the first leftover frame (if any)
        if let Some((_, _, channel_count, buf))
        = leftovers.get_mut(0) {
            let mut i = 0;
            let fade_len = (buf.len() / *channel_count as usize).min(1000);
            let fade_mult = 1.0 / (fade_len as f32);
            for n in 1..fade_len-1 {
                let volume = (n as f32) * fade_mult;
                for _ in 0..*channel_count as usize {
                    buf[i] *= volume;
                    i += 1;
                }
            }
        }
        self.leftovers = leftovers;
    }
}

impl Drop for AVFormat {
    fn drop(&mut self) {
        self.close_input();
    }
}

fn expand_float_packed_audio(frame: &ff::AVFrame, buf: &mut Vec<f32>){
    let data_ptr: &[f32] = unsafe {
        std::slice::from_raw_parts(transmute(frame.extended_data.read()),
                                   frame.nb_samples as usize
                                   * frame.channels as usize)
    };
    buf.extend_from_slice(data_ptr);
}

fn expand_float_planar_audio(frame: &ff::AVFrame, buf: &mut Vec<f32>) {
    assert!(frame.channels >= 1);
    let mut data_ptrs: Vec<&[f32]> = Vec::with_capacity(frame.channels as usize);
    for c in 0 .. frame.channels as usize {
        data_ptrs.push(unsafe {
            let raw_ptr = frame.extended_data.offset(c as isize).read();
            std::slice::from_raw_parts(transmute(raw_ptr),
                                       frame.nb_samples as usize)
        });
    }
    for q in 0 .. frame.nb_samples as usize {
        for c in 0 .. frame.channels as usize {
            buf.push(data_ptrs[c][q])
        }
    }
}

fn expand_packed_audio<T: Expandable>(frame: &ff::AVFrame, buf: &mut Vec<f32>){
    let data_ptr: &[T] = unsafe {
        std::slice::from_raw_parts(transmute(frame.extended_data.read()),
                                   frame.nb_samples as usize
                                   * frame.channels as usize)
    };
    for q in 0 .. (frame.nb_samples * frame.channels) as usize {
        buf.push(data_ptr[q].expanded());
    }
}

fn expand_planar_audio<T: Expandable>(frame: &ff::AVFrame, buf: &mut Vec<f32>){
    assert!(frame.channels >= 1);
    let mut data_ptrs: Vec<&[T]> = Vec::with_capacity(frame.channels as usize);
    for c in 0 .. frame.channels as usize {
        data_ptrs.push(unsafe {
            let raw_ptr = frame.extended_data.offset(c as isize).read();
            std::slice::from_raw_parts(transmute(raw_ptr),
                                       frame.nb_samples as usize)
        });
    }
    for q in 0 .. frame.nb_samples as usize {
        for c in 0 .. frame.channels as usize {
            buf.push(data_ptrs[c][q].expanded())
        }
    }
}

// A primitive data type that is used to encode audio, and knows how to convert
// into the standard floating point encoding.
trait Expandable {
    fn expanded(&self) -> f32;
}

impl Expandable for u8 {
    fn expanded(&self) -> f32 {
        (*self as f32 / 127.5) - 1.0
    }
}

impl Expandable for i16 {
    fn expanded(&self) -> f32 {
        *self as f32 / i16::MAX as f32
    }
}

impl Expandable for i32 {
    fn expanded(&self) -> f32 {
        *self as f32 / i32::MAX as f32
    }
}

impl Expandable for i64 {
    fn expanded(&self) -> f32 {
        *self as f32 / i64::MAX as f32
    }
}

impl Expandable for f64 {
    // bit of a misnomer in this case...
    fn expanded(&self) -> f32 {
        *self as f32
    }
}

/// Call once, at launch time, to do basic initialization of FFMPEG.
pub fn init() {
    unsafe {
        ff::av_log_set_callback(Some(ffmpeg_log_stub));
    }
}

extern {
    fn ffmpeg_log_stub(p: *mut libc::c_void, level: libc::c_int,
                       format: *const libc::c_char,
                       args: *mut ff::__va_list_tag);
}

#[no_mangle]
pub extern "C" fn ffmpeg_log_backend(_p: libc::uintptr_t, level: libc::c_int,
                                     text: *const libc::c_char) {
    let text = unsafe { std::ffi::CStr::from_ptr(text) };
    let text = text.to_string_lossy().into_owned();
    let mut text = &text[..];
    while text.ends_with("\r") || text.ends_with("\n") {
        text = &text[..text.len()-1];
    }
    let level = level as u32;
    if level >= ff::AV_LOG_DEBUG { trace!("{}", text) }
    else if level >= ff::AV_LOG_VERBOSE { debug!("{}", text) }
    else if level >= ff::AV_LOG_INFO { info!("{}", text) }
    else if level >= ff::AV_LOG_WARNING { warn!("{}", text) }
    else { error!("{}", text) }
}
