#![allow(clippy::uninit_assumed_init)]
#![allow(invalid_value)]

use crate::{
    ffi::{mpv_format, mpv_free, mpv_get_property},
    log_code, CTX,
};
use std::{
    ffi::{c_char, c_int, CStr},
    mem::MaybeUninit,
    ptr::addr_of_mut,
};

pub fn get_property_f64(name: &CStr) -> Option<f64> {
    let mut data = unsafe { MaybeUninit::<f64>::uninit().assume_init() };
    let error = unsafe {
        mpv_get_property(
            CTX,
            name.as_ptr(),
            mpv_format::MPV_FORMAT_DOUBLE,
            addr_of_mut!(data).cast(),
        )
    };
    if error < 0 {
        log_code(error);
        None
    } else {
        Some(data)
    }
}

pub fn get_property_bool(name: &CStr) -> Option<bool> {
    let mut data = unsafe { MaybeUninit::<c_int>::uninit().assume_init() };
    let error = unsafe {
        mpv_get_property(
            CTX,
            name.as_ptr(),
            mpv_format::MPV_FORMAT_FLAG,
            addr_of_mut!(data).cast(),
        )
    };
    if error < 0 {
        log_code(error);
        None
    } else {
        Some(data != 0)
    }
}

pub fn get_property_string(name: &CStr) -> Option<String> {
    let mut data = unsafe { MaybeUninit::<*mut c_char>::uninit().assume_init() };
    let error = unsafe {
        mpv_get_property(
            CTX,
            name.as_ptr(),
            mpv_format::MPV_FORMAT_STRING,
            addr_of_mut!(data).cast(),
        )
    };
    if error < 0 {
        log_code(error);
        None
    } else {
        let value = unsafe { CStr::from_ptr(data) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { mpv_free(data.cast()) };
        Some(value)
    }
}
