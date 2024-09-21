use crate::options::Filter;
use anyhow::{anyhow, Result};
use hex::encode;
use md5::{Digest, Md5};
use reqwest::Client;
use serde::{Deserialize, Deserializer};
use serde::de::{self, Visitor, SeqAccess};
use std::{fmt,hint};
use std::sync::{Arc, LazyLock};


// 定义全局的 HTTP 客户端
static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

pub struct StatusInner {
    pub x: f64,
    pub row: usize,
    pub step: f64,
}

pub enum Status {
    Status(StatusInner),
    Overlapping,
    Uninitialized,
}

impl Status {
    pub fn insert(&mut self, status: StatusInner) -> &mut StatusInner {
        *self = Status::Status(status);
        match self {
            Status::Status(status) => status,
            _ => unsafe { hint::unreachable_unchecked() },
        }
    }
}

pub struct Danmaku {
    pub message: String,
    pub count: usize,
    pub time: f64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub source: Source,
    pub blocked: bool,
    pub status: Status,
}



#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Source {
    Bilibili,
    Gamer,
    AcFun,
    QQ,
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
            "qq" => Source::QQ,
            "iqiyi" => Source::IQIYI,
            "d" => Source::D,
            "dandan" => Source::Dandan,
            _ => Source::Unknown,
        }
    }
}



// 定义用于解析搜索响应的结构体
#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Data,
}

#[derive(Debug, Deserialize)]
struct Data {
    longData: Option<LongData>,
}

#[derive(Debug, Deserialize)]
struct LongData {
    rows: Vec<Row>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Row {
    Series(SeriesRow),
    Movie(MovieRow),
    Show(ShowRow),
    // 可以添加更多的变体
}

#[derive(Debug, Deserialize)]
struct SeriesRow {
    #[serde(deserialize_with = "deserialize_playlinks")]
    seriesPlaylinks: Vec<Playlink>,
}

#[derive(Debug, Deserialize)]
struct MovieRow {
    playlinks: Playlinks,
}

#[derive(Debug, Deserialize)]
struct ShowRow {
    id: String,
    year: String,
    vipSite: Vec<String>,
    playlinks_total: PlaylinksTotal,
}

#[derive(Debug, Deserialize)]
struct Playlink {
    url: String,
    c: String,
}

#[derive(Debug, Deserialize)]
struct Playlinks {
    bilibili1: Option<String>,
    imgo: Option<String>,
    qiyi: Option<String>,
    qq: Option<String>,
    youku: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlaylinksTotal {
    bilibili1: Option<u32>,
    imgo: Option<u32>,
    qiyi: Option<u32>,
    qq: Option<u32>,
    youku: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ShowsApiResponse {
    data: ShowsApiData,
}

#[derive(Debug, Deserialize)]
struct ShowsApiData {
    list: Vec<ShowItem>,
}

#[derive(Debug, Deserialize)]
struct ShowItem {
    url: String,
}

#[derive(Debug, Deserialize)]
struct DanmakuResponse {
    danmuku: Vec<DanmakuItem>,
}

#[derive(Debug, Deserialize)]
struct DanmakuItem(
    f64,    // time
    u8,     // type (ignored)
    String, // color
    String, // message
    String, // user
);

// 自定义反序列化函数，用于处理可能为字符串或对象的 playlinks
fn deserialize_playlinks<'de, D>(deserializer: D) -> Result<Vec<Playlink>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PlaylinkVisitor;

    impl<'de> Visitor<'de> for PlaylinkVisitor {
        type Value = Vec<Playlink>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of Playlink objects or strings")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut playlinks = Vec::new();

            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                match value {
                    serde_json::Value::Object(obj) => {
                        let playlink: Playlink =
                            serde_json::from_value(serde_json::Value::Object(obj))
                                .map_err(de::Error::custom)?;
                        playlinks.push(playlink);
                    }
                    serde_json::Value::String(url) => {
                        playlinks.push(Playlink {
                            url,
                            c: "".to_string(),
                        });
                    }
                    _ => {
                        return Err(de::Error::custom("Unexpected value in seriesPlaylinks"));
                    }
                }
            }

            Ok(playlinks)
        }
    }

    deserializer.deserialize_seq(PlaylinkVisitor)
}


// 辅助结构体
struct SearchQuery {
    title: String,
    season_number: Option<usize>,
    episode_number: Option<usize>,
}

// 解析名称的函数
fn parse_name(name: &str) -> Result<SearchQuery> {
    let parts: Vec<&str> = name.split(['-', ' ']).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(anyhow!("Invalid input format: parts is empty"));
    }

    let title = parts[0].to_string();
    let mut season_number = None;
    let mut episode_number = None;

    if parts.len() >= 2 {
        let separts: Vec<&str> = parts[1]
            .split(['s', 'e', ':', '-', 'S', 'E', ' '])
            .filter(|s| !s.is_empty())
            .collect();
        if separts.len() >= 2 {
            season_number = Some(separts[0].parse()?);
            episode_number = Some(separts[1].parse()?);
        }
    }

    Ok(SearchQuery {
        title,
        season_number,
        episode_number,
    })
}

// 构建搜索 URL 的函数
fn construct_search_url(query: &SearchQuery) -> String {
    if let Some(season_number) = query.season_number {
        format!(
            "https://api.so.360kan.com/index?force_v=1&kw={}{}&from=&pageno=1&v_ap=1&tab=all",
            query.title, season_number
        )
    } else {
        format!(
            "https://api.so.360kan.com/index?force_v=1&kw={}&from=&pageno=1&v_ap=1&tab=all",
            query.title
        )
    }
}

// 提取播放链接的函数
async fn extract_play_url(
    search_response: &SearchResponse,
    episode_number: usize,
) -> Result<String> {
    let long_data = search_response
        .data
        .longData
        .as_ref()
        .ok_or_else(|| anyhow!("Cannot find the series"))?;

    let first_row = long_data
        .rows
        .get(0)
        .ok_or_else(|| anyhow!("Cannot find the series"))?;

    match first_row {
        Row::Series(series_row) => {
            if episode_number > series_row.seriesPlaylinks.len() {
                return Err(anyhow!("Episode number out of range"));
            }
            Ok(series_row.seriesPlaylinks[episode_number - 1].url.clone())
        }
        Row::Movie(movie_row) => {
            movie_row
                .playlinks
                .bilibili1
                .clone()
                .or_else(|| movie_row.playlinks.qiyi.clone())
                .or_else(|| movie_row.playlinks.qq.clone())
                .or_else(|| movie_row.playlinks.youku.clone())
                .or_else(|| movie_row.playlinks.imgo.clone())
                .ok_or_else(|| anyhow!("No links available"))
        }
        Row::Show(show_row) => {
            extract_play_url_from_show(show_row, episode_number).await
        }
        _ => Err(anyhow!("First row does not contain valid playlinks")),
    }
}

// 处理 Row::Show 的辅助函数
async fn extract_play_url_from_show(
    show_row: &ShowRow,
    episode_number: usize,
) -> Result<String> {
    let fields = vec![
        ("bilibili1", show_row.playlinks_total.bilibili1),
        ("imgo", show_row.playlinks_total.imgo),
        ("qiyi", show_row.playlinks_total.qiyi),
        ("qq", show_row.playlinks_total.qq),
        ("youku", show_row.playlinks_total.youku),
    ];

    // 过滤出有值的字段名
    let vipsites: Vec<&str> = fields
        .into_iter()
        .filter_map(|(name, value)| {
            if value.is_some() {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    let vipsite = vipsites
        .get(0)
        .ok_or_else(|| anyhow!("Cannot find the vipsite"))?;

    let year = show_row
        .year
        .parse::<i32>()
        .map_err(|_| anyhow!("Invalid year format"))?;

    let entid = show_row
        .id
        .parse::<i32>()
        .map_err(|_| anyhow!("Invalid id format"))?;

    let total_number = show_row
        .playlinks_total
        .bilibili1
        .clone()
        .or_else(|| show_row.playlinks_total.qq.clone())
        .or_else(|| show_row.playlinks_total.youku.clone())
        .or_else(|| show_row.playlinks_total.qiyi.clone())
        .or_else(|| show_row.playlinks_total.imgo.clone())
        .unwrap_or(0);

    if episode_number > total_number as usize {
        return Err(anyhow!("Episode number out of range"));
    }

    let offset = (total_number as usize) - episode_number;
    let url = format!(
        "https://api.so.360kan.com/episodeszongyi?site={}&y={}&entid={}&offset={}&count=8&v_ap=1",
        vipsite, year, entid, offset
    );

    let shows_response: ShowsApiResponse = CLIENT
        .get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?
        .json()
        .await?;

    let play_url = shows_response
        .data
        .list
        .get(0)
        .map(|item| item.url.clone())
        .ok_or_else(|| anyhow!("Cannot find the series"))?;

    Ok(play_url)
}

// 获取并处理弹幕数据的函数
async fn fetch_and_process_danmaku(
    play_url: &str,
    filter: Arc<Filter>,
) -> Result<Vec<Danmaku>> {
    let danmaku_url = format!("https://danmu.zxz.ee/?type=json&id={}", play_url);
    let danmaku_response: DanmakuResponse = CLIENT
        .get(&danmaku_url)
        .send()
        .await?
        .json()
        .await?;

    process_danmaku_response(danmaku_response, filter).await
}

// 处理弹幕响应的函数
async fn process_danmaku_response(
    danmaku_response: DanmakuResponse,
    filter: Arc<Filter>,
) -> Result<Vec<Danmaku>> {
    let sources_rt = filter.sources_rt.lock().await;

    let mut danmaku_list = danmaku_response
        .danmuku
        .into_iter()
        .filter(|item| filter.keywords.iter().all(|pat| !item.3.contains(pat)))
        .map(|item| {
            let cmessage = item.3;
            let ccount = cmessage.chars().count();
            let color = u32::from_str_radix(&item.2[1..], 16).unwrap_or(0);
            let user = item.4;
            let source = if user.chars().all(char::is_numeric) {
                Source::Dandan
            } else {
                user.strip_prefix('[')
                    .and_then(|user| user.split_once(']').map(|(source, _)| source.into()))
                    .unwrap_or(Source::Unknown)
            };
            Danmaku {
                time: item.0,
                message: cmessage,
                count: ccount,
                r: ((color >> 16) & 0xFF) as u8,
                g: ((color >> 8) & 0xFF) as u8,
                b: (color & 0xFF) as u8,
                source,
                blocked: sources_rt
                    .as_ref()
                    .map(|s| s.contains(&source))
                    .unwrap_or_else(|| filter.sources.contains(&source)),
                status: Status::Uninitialized,
            }
        })
        .collect::<Vec<_>>();

    danmaku_list.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    Ok(danmaku_list)
}

// 重构后的 get_danmaku 函数
pub async fn get_danmaku(name: &str, filter: Arc<Filter>) -> Result<Vec<Danmaku>> {
    let query = parse_name(name)?;
    let episode_number = query.episode_number.unwrap_or(1);
    let search_url = construct_search_url(&query);

    let search_response: SearchResponse = CLIENT
        .get(&search_url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?
        .json()
        .await?;

    let play_url = extract_play_url(&search_response, episode_number).await?;
    fetch_and_process_danmaku(&play_url, filter).await
}

// 重构后的 get_danmaku_byurl 函数
pub async fn get_danmaku_byurl(url: &str, filter: Arc<Filter>) -> Result<Vec<Danmaku>> {
    fetch_and_process_danmaku(url, filter).await
}