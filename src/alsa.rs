//! This module is used to suppress ALSA lib log outputs on Linux. It's
//! necessary due to a longstanding bug in PortAudio:
//!
//! <https://app.assembla.com/spaces/portaudio/tickets/163>

use log::debug;

extern {
    fn alsa_log_stub(file: *const libc::c_char,
                     line: libc::c_int,
                     function: *const libc::c_char,
                     errno: libc::c_int,
                     fmt: *const libc::c_char,
                     ...);
}

#[no_mangle]
pub extern "C" fn alsa_log_backend(_file: *const libc::c_char,
                                   _line: libc::c_int,
                                   _function: *const libc::c_char,
                                   _errno: libc::c_int,
                                   text: *const libc::c_char) {
    let text = unsafe { std::ffi::CStr::from_ptr(text) };
    let text = text.to_string_lossy().into_owned();
    let mut text = &text[..];
    while text.ends_with("\r") || text.ends_with("\n") {
        text = &text[..text.len()-1];
    }
    debug!("{}", text);
}

pub fn suppress_logs() {
    unsafe {
        alsa_sys::snd_lib_error_set_handler(Some(alsa_log_stub));
    }
}
