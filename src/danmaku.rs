use crate::Filter;
use anyhow::{anyhow, Result};
use hex::encode;
use lazy_static::lazy_static;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{copy, Read},
    path::Path,
};
use unicode_segmentation::UnicodeSegmentation;

pub struct Danmaku {
    pub message: String,
    pub count: usize,
    pub time: f64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub source: Source,
    pub x: Option<f64>,
    pub row: Option<usize>,
    pub blocked: bool,
}

#[derive(Deserialize)]
struct MatchResponse {
    #[serde(rename = "isMatched")]
    is_matched: bool,
    matches: Vec<Match>,
}

#[derive(Deserialize)]
struct Match {
    #[serde(rename = "episodeId")]
    episode_id: usize,
}

#[derive(Deserialize)]
struct CommentResponse {
    comments: Vec<Comment>,
}

#[derive(Deserialize)]
struct Comment {
    p: String,
    m: String,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Source {
    Bilibili,
    Gamer,
    AcFun,
    Tencent,
    IQIYI,
    D,
    Dandan,
    Unknown,
}

impl From<&str> for Source {
    fn from(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "bilibili" => Source::Bilibili,
            "gamer" => Source::Gamer,
            "acfun" => Source::AcFun,
            "qq" => Source::Tencent,
            "iqiyi" => Source::IQIYI,
            "d" => Source::D,
            "dandan" => Source::Dandan,
            _ => Source::Unknown,
        }
    }
}

lazy_static! {
    static ref CLIENT: Client = Client::new();
}

pub async fn get_danmaku<P: AsRef<Path>>(path: P, filter: Filter) -> Result<Vec<Danmaku>> {
    let file = File::open(&path)?;
    let mut hasher = Md5::new();
    // https://api.dandanplay.net/swagger/ui/index
    copy(&mut file.take(16 * 1024 * 1024), &mut hasher)?;
    let hash = encode(hasher.finalize());
    let file_name = path.as_ref().file_name().unwrap().to_str().unwrap();

    let data = CLIENT
        .post("https://api.dandanplay.net/api/v2/match")
        .header("Content-Type", "application/json")
        .json(&HashMap::from([
            ("fileName", file_name),
            ("fileHash", &hash),
        ]))
        .send()
        .await?
        .json::<MatchResponse>()
        .await?;
    if data.matches.len() > 1 {
        return Err(anyhow!("multiple matching episodes"));
    } else if !data.is_matched {
        return Err(anyhow!("no matching episode"));
    }

    let danmaku = CLIENT
        .get(format!(
            "https://api.dandanplay.net/api/v2/comment/{}?withRelated=true",
            data.matches[0].episode_id
        ))
        .send()
        .await?
        .json::<CommentResponse>()
        .await?
        .comments;
    let sources_rt = filter.sources_rt.lock().await;
    let mut danmaku = danmaku
        .into_iter()
        .filter(|comment| filter.keywords.iter().all(|pat| !comment.m.contains(pat)))
        .map(|comment| {
            let mut p = comment.p.splitn(4, ',');
            let time = p.next().unwrap().parse().unwrap();
            _ = p.next().unwrap();
            let color = p.next().unwrap().parse::<u32>().unwrap();
            let user = p.next().unwrap();
            let source = if user.chars().all(char::is_numeric) {
                Source::Dandan
            } else {
                user.strip_prefix('[')
                    .and_then(|user| user.split_once(']').map(|(source, _)| source.into()))
                    .unwrap_or(Source::Unknown)
            };
            Danmaku {
                message: comment.m.replace('\n', "\\N"),
                count: comment.m.graphemes(true).count(),
                time,
                r: (color / (256 * 256)).try_into().unwrap(),
                g: (color % (256 * 256) / 256).try_into().unwrap(),
                b: (color % 256).try_into().unwrap(),
                source,
                x: None,
                row: None,
                blocked: sources_rt
                    .as_ref()
                    .map(|s| s.contains(&source))
                    .unwrap_or_else(|| filter.sources.contains(&source)),
            }
        })
        .collect::<Vec<_>>();

    danmaku.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    Ok(danmaku)
}
