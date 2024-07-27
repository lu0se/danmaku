use crate::{mpv::expand_path, CLIENT_NAME};
use anyhow::Result;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, ErrorKind},
};

pub fn read_options() -> Result<Option<HashMap<String, String>>> {
    let path = format!("~~/script-opts/{}.conf", unsafe { CLIENT_NAME });
    let file = match File::open(expand_path(&path)?) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut opts = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.starts_with('#') {
            if let Some((k, v)) = line.split_once('=') {
                opts.insert(k.into(), v.into());
            }
        }
    }
    Ok(Some(opts))
}
