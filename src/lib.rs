pub mod danmaku;
pub mod ffi;
pub mod log;
pub mod mpv;
pub mod options;

use crate::{
    danmaku::{get_danmaku, Danmaku, Source},
    ffi::{
        mpv_client_name, mpv_event_client_message, mpv_event_id, mpv_format, mpv_handle,
        mpv_observe_property, mpv_wait_event,
    },
    log::{log_code, log_error},
    mpv::{
        expand_path, get_property_bool, get_property_f64, get_property_string, osd_message,
        osd_overlay, remove_overlay,
    },
    options::read_options,
};
use anyhow::{anyhow, Result};
use ffi::{mpv_event_property, mpv_node};
use serde::Deserialize;
use std::{
    cmp::max,
    collections::HashSet,
    ffi::CStr,
    fs::File,
    io::BufReader,
    os::raw::c_int,
    ptr::null_mut,
    slice::from_raw_parts,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{runtime::Builder, spawn, sync::Mutex};

const DURATION: f64 = 12.;
const INTERVAL: f64 = 0.005;

pub static mut CTX: *mut mpv_handle = null_mut();
pub static mut CLIENT_NAME: &str = "";

#[derive(Deserialize)]
struct BilibiliFilterRule {
    r#type: usize,
    filter: String,
    opened: bool,
}

#[derive(Clone, Copy)]
struct Options {
    font_size: f64,
    transparency: u8,
    reserved_space: f64,
    delay: f64,
}

#[derive(Clone)]
pub struct Filter {
    keywords: Arc<Vec<String>>,
    sources: Arc<HashSet<Source>>,
    sources_rt: Arc<Mutex<Option<HashSet<Source>>>>,
}

#[no_mangle]
extern "C" fn mpv_open_cplugin(ctx: *mut mpv_handle) -> c_int {
    unsafe {
        CTX = ctx;
        CLIENT_NAME = CStr::from_ptr(mpv_client_name(ctx)).to_str().unwrap();
    }

    Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(main(ctx))
}

async fn main(ctx: *mut mpv_handle) -> c_int {
    let options = read_options()
        .map_err(log_error)
        .ok()
        .flatten()
        .unwrap_or_default();

    let mut keywords: Vec<_> = options
        .get("filter")
        .filter(|f| !f.is_empty())
        .map(|f| f.split(',').map(Into::into).collect())
        .unwrap_or_default();
    if let Some(file) = options
        .get("filter_bilibili")
        .and_then(|f| expand_path(f).map_err(log_error).ok())
    {
        match (|| -> Result<_> {
            Ok(serde_json::from_reader::<_, Vec<BilibiliFilterRule>>(
                BufReader::new(File::open(file)?),
            )?)
        })() {
            Ok(rules) => keywords.extend(
                rules
                    .into_iter()
                    .filter(|r| r.r#type == 0 && r.opened)
                    .map(|r| r.filter),
            ),
            Err(error) => log_error(anyhow!("option filter_bilibili: {}", error)),
        }
    }
    let filter = Filter {
        keywords: Arc::new(keywords),
        sources: Arc::new(
            options
                .get("filter_source")
                .filter(|f| !f.is_empty())
                .map(|f| {
                    f.split(',')
                        .map(Into::into)
                        .filter(|&s| s != Source::Unknown)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        sources_rt: Arc::new(Mutex::new(None)),
    };

    let mut options = Options {
        font_size: options
            .get("font_size")
            .and_then(|s| s.parse().ok().filter(|&s| s > 0.))
            .unwrap_or(40.),
        transparency: options
            .get("transparency")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0x30),
        reserved_space: options
            .get("reserved_space")
            .and_then(|r| r.parse().ok().filter(|r| (0. ..1.).contains(r)))
            .unwrap_or(0.),
        delay: 0.,
    };

    for (name, format) in [
        (c"script-opts", mpv_format::MPV_FORMAT_NODE),
        (c"pause", mpv_format::MPV_FORMAT_FLAG),
    ] {
        let error = unsafe { mpv_observe_property(ctx, 0, name.as_ptr(), format) };
        if error < 0 {
            log_code(error);
            return -1;
        }
    }

    let comments = Arc::new(Mutex::new(None));
    let enabled = Arc::new(AtomicBool::new(false));
    let mut handle = spawn(async {});
    let mut pause = true;
    loop {
        let timeout = if !pause && enabled.load(Ordering::SeqCst) {
            INTERVAL
        } else {
            -1.
        };
        let event = unsafe { &*mpv_wait_event(ctx, timeout) };
        match event.event_id {
            mpv_event_id::MPV_EVENT_SHUTDOWN => {
                handle.abort();
                return 0;
            }
            mpv_event_id::MPV_EVENT_FILE_LOADED => {
                handle.abort();
                *comments.lock().await = None;
                options.delay = 0.;
                if enabled.load(Ordering::SeqCst) {
                    remove_overlay();
                    handle = spawn(get(
                        comments.clone(),
                        enabled.clone(),
                        filter.clone(),
                        options,
                    ));
                }
            }
            mpv_event_id::MPV_EVENT_SEEK => {
                if enabled.load(Ordering::SeqCst) {
                    if let Some(comments) = &mut *comments.lock().await {
                        reset(comments);
                    }
                }
            }
            mpv_event_id::MPV_EVENT_PROPERTY_CHANGE => {
                let data = unsafe { &*(event.data as *mut mpv_event_property) };
                let name = unsafe { CStr::from_ptr(data.name) };
                if name == c"script-opts" && data.format == mpv_format::MPV_FORMAT_NODE {
                    let data = unsafe { &*(data.data as *mut mpv_node) };
                    assert_eq!(data.format, mpv_format::MPV_FORMAT_NODE_MAP);
                    let list = unsafe { &*data.u.list };
                    let num = list.num.try_into().unwrap();
                    let keys = unsafe { from_raw_parts(list.keys, num) };
                    let values = unsafe { from_raw_parts(list.values, num) };
                    for (key, value) in keys.iter().zip(values) {
                        if unsafe {
                            CStr::from_ptr(key.cast())
                                .to_str()
                                .map(|key| key == format!("{}-filter_source", CLIENT_NAME))
                                .unwrap_or(false)
                        } {
                            assert_eq!(value.format, mpv_format::MPV_FORMAT_STRING);
                            match unsafe { CStr::from_ptr(value.u.string) }.to_str() {
                                Ok(value) => {
                                    *filter.sources_rt.lock().await = if value.is_empty() {
                                        if let Some(comments) = &mut *comments.lock().await {
                                            for comment in comments {
                                                comment.blocked =
                                                    filter.sources.contains(&comment.source);
                                            }
                                        }
                                        osd_message(&format!(
                                            "Danmaku: blocked danmaku from {:?}",
                                            filter.sources
                                        ));
                                        None
                                    } else {
                                        let sources = value
                                            .split(',')
                                            .map(Into::into)
                                            .filter(|&s| s != Source::Unknown)
                                            .collect::<HashSet<_>>();
                                        if let Some(comments) = &mut *comments.lock().await {
                                            for comment in comments {
                                                comment.blocked = sources.contains(&comment.source);
                                            }
                                        }
                                        osd_message(&format!(
                                            "Danmaku: blocked danmaku from {:?}",
                                            sources
                                        ));
                                        Some(sources)
                                    }
                                }
                                Err(error) => log_error(error.into()),
                            }
                            break;
                        }
                    }
                } else if name == c"pause" && data.format == mpv_format::MPV_FORMAT_FLAG {
                    pause = unsafe { *(data.data as *mut c_int) } != 0;
                }
            }
            mpv_event_id::MPV_EVENT_CLIENT_MESSAGE => 'a: {
                let data = unsafe { &*(event.data as *mut mpv_event_client_message) };
                if data.args.is_null() {
                    break 'a;
                }
                if let [arg1, args @ ..] =
                    unsafe { from_raw_parts(data.args, data.num_args.try_into().unwrap()) }
                {
                    let arg1 = unsafe { CStr::from_ptr(*arg1) };
                    if arg1 == c"toggle-danmaku" {
                        if enabled.fetch_xor(true, Ordering::SeqCst) {
                            remove_overlay();
                            osd_message("Danmaku: off");
                        } else {
                            match &mut *comments.lock().await {
                                Some(comments) => {
                                    reset(comments);
                                    loaded(comments);
                                }
                                None => {
                                    osd_message("Danmaku: on");
                                    handle.abort();
                                    handle = spawn(get(
                                        comments.clone(),
                                        enabled.clone(),
                                        filter.clone(),
                                        options,
                                    ));
                                }
                            }
                        }
                    } else if arg1 == c"danmaku-delay" {
                        match args.first().map(|&arg| unsafe { CStr::from_ptr(arg) }) {
                            Some(seconds) => {
                                match seconds.to_str().ok().and_then(|s| s.parse::<f64>().ok()) {
                                    Some(seconds) => {
                                        options.delay += seconds;
                                        if let Some(comments) = &mut *comments.lock().await {
                                            reset(comments);
                                        }
                                        osd_message(&format!(
                                            "Danmaku delay: {:.0} ms",
                                            options.delay * 1000.
                                        ));
                                    }
                                    None => {
                                        log_error(anyhow!("command danmaku-delay: invalid time"))
                                    }
                                }
                            }
                            None => log_error(anyhow!(
                                "command danmaku-delay: required argument seconds not set"
                            )),
                        }
                    }
                }
            }
            _ => (),
        }

        if enabled.load(Ordering::SeqCst) {
            if let Some(comments) = &mut *comments.lock().await {
                render(comments, options);
            }
        }
    }
}

fn render(comments: &mut [Danmaku], options: Options) -> Option<()> {
    let mut width = 1920.;
    let mut height = 1080.;
    let ratio = get_property_f64(c"osd-width")? / get_property_f64(c"osd-height")?;
    if ratio > width / height {
        height = width / ratio;
    } else if ratio < width / height {
        width = height * ratio;
    }
    let pos = get_property_f64(c"time-pos")?;
    let speed = get_property_f64(c"speed")?;
    let spacing = options.font_size / 10.;
    let mut ends = Vec::new();
    ends.resize(
        max(
            (height * (1. - options.reserved_space) / (options.font_size + spacing)) as usize,
            1,
        ),
        None,
    );

    let mut danmaku = Vec::new();
    for comment in comments.iter_mut().filter(|c| !c.blocked) {
        let time = comment.time + options.delay;
        if time > pos + DURATION / 2. {
            break;
        }

        let x = comment
            .x
            .get_or_insert_with(|| width - (pos - time) * width / DURATION);
        if *x + comment.count as f64 * options.font_size + spacing < 0. {
            continue;
        }
        let row = *comment.row.get_or_insert_with(|| {
            ends.iter()
                .enumerate()
                .find(|(_, end)| end.map(|end: f64| end < *x).unwrap_or(true))
                .map(|(row, _)| row)
                .unwrap_or_else(|| {
                    ends.iter()
                        .enumerate()
                        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                        .map(|(row, _)| row)
                        .unwrap()
                })
        });
        danmaku.push(format!(
            "{{\\pos({},{})\\c&H{:x}{:x}{:x}&\\alpha&H{:x}\\fs{}\\bord1.5\\shad0\\b1\\q2}}{}",
            *x,
            row as f64 * (options.font_size + spacing),
            comment.b,
            comment.g,
            comment.r,
            options.transparency,
            options.font_size,
            comment.message
        ));

        *x -= width / DURATION * speed * INTERVAL;
        if let Some(end) = ends.get_mut(row) {
            let new_end = *x + comment.count as f64 * options.font_size + spacing;
            match end {
                Some(end) => *end = end.max(new_end),
                None => *end = Some(new_end),
            }
        }
    }
    osd_overlay(&danmaku.join("\n"), width as i64, height as i64);
    Some(())
}

async fn get(
    comments: Arc<Mutex<Option<Vec<Danmaku>>>>,
    enabled: Arc<AtomicBool>,
    filter: Filter,
    options: Options,
) {
    let Some(path) = get_property_string(c"path") else {
        return;
    };
    match get_danmaku(path, filter).await {
        Ok(mut danmaku) => {
            if enabled.load(Ordering::SeqCst) {
                if let Some(true) = get_property_bool(c"pause") {
                    render(&mut danmaku, options);
                }
                loaded(&danmaku);
            }
            *comments.lock().await = Some(danmaku)
        }
        Err(error) => {
            if enabled.load(Ordering::SeqCst) {
                osd_message(&format!("Danmaku: {}", error));
            }
            log_error(error);
        }
    }
}

fn reset(comments: &mut Vec<Danmaku>) {
    for comment in comments {
        comment.x = None;
        comment.row = None;
    }
}

fn loaded(comments: &[Danmaku]) {
    let n = comments.iter().filter(|c| !c.blocked).count();
    osd_message(&format!(
        "Loaded {} danmaku comment{}",
        n,
        if n > 1 { "s" } else { "" }
    ));
}
