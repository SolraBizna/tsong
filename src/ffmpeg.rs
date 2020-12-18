//! Safe bindings to the subset of the ffmpeg-dev crate that we use.

use anyhow::anyhow;
use ffmpeg_dev::sys as ff;
use ffmpeg_dev::extra::defs as ffdefs;
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString},
    path::Path,
    ptr::null_mut,
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

/// Wraps an (input!) AVFormatContext
pub struct AVFormat {
    inner: *mut ff::AVFormatContext,
}

impl AVFormat {
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
        Ok(AVFormat { inner })
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
    pub fn find_best_stream(&mut self) -> anyhow::Result<Option<libc::c_int>> {
        assert!(!self.inner.is_null());
        match unsafe { ff::av_find_best_stream(self.inner,
                                            ff::AVMediaType_AVMEDIA_TYPE_AUDIO,
                                               -1, -1, null_mut(),
                                               0) } {
            x if x >= 0 => Ok(Some(x)),
            x if x == unsafe { ffdefs::averror_stream_not_found() }
                => Ok(None),
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
            if stream >= 0 && (stream as u32) < inner.nb_streams {
                let stream_ref = unsafe {
                    inner.streams.offset(stream as isize).read().as_ref()
                };
                if let Some(stream_ref) = stream_ref {
                    transcribe_dict(&mut ret, stream_ref.metadata);
                }
            }
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
        else { panic!("invalid stream index: {}", stream); }
    }
    /// Calls `avformat_close_input`. This gets called automatically if this
    /// context is dropped without being closed.
    pub fn close_input(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                ff::avformat_close_input(&mut self.inner);
            }
            self.inner = null_mut();
        }
    }
}

impl Drop for AVFormat {
    fn drop(&mut self) {
        self.close_input();
    }
}
