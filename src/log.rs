use crate::{ffi::mpv_error_string, CLIENT_NAME};
use anyhow::Error;
use std::ffi::{c_int, CStr};

pub fn log_code(error: c_int) {
    unsafe {
        eprintln!(
            "[{CLIENT_NAME}] {}",
            CStr::from_ptr(mpv_error_string(error)).to_str().unwrap()
        )
    }
}

pub fn log_error(error: Error) {
    unsafe { eprintln!("[{CLIENT_NAME}] {error}") }
}
