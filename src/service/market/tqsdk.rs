//! TqSdk (天勤量化 / Shinny 快期) market data provider for Chinese futures contracts (`SHFE`,
//! `DCE`, `CZCE`, `CFFEX`, `INE`, `GFEX`, plus `KQ.m@` main continuous and `KQ.i@` index contracts).
//!
//! Uses Shinny's OAuth2 token endpoint (`https://auth.shinnytech.com`) and DIFF WebSocket protocol
//! (`ins_query`, `set_chart`, `subscribe_quote`, `peek_message`) over `tungstenite`.

use super::catalog::Catalog;
use super::error::ProviderError;
use crate::lock::lock;
use crate::model::{DataSource, Timeframe};
use crate::model::watchlist::quote::{SourceQuote, VolumeKind};
use crate::service::keychain::api_keys::tqsdk_credentials;
use crate::service::net::http as net_http;
use crate::service::watchlist_quotes::{QuoteFailure, QuoteRequest};
use crate::service::{Kline, MarketSource, NAME_SEPARATOR, SearchResult};
use crate::service::market::candles::plan;
use flate2::{Decompress, FlushDecompress, Status};
use http::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tungstenite::WebSocket;
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;

const AUTH_URL: &str =
    "https://auth.shinnytech.com/auth/realms/shinnytech/protocol/openid-connect/token";
const NS_URL: &str = "https://api.shinnytech.com/ns?stock=false&backtest=false";
const DEFAULT_MD_URL: &str = "wss://free-openmd.shinnytech.com/t/md/front/mobile";
const CLIENT_ID: &str = "shinny_tq";
const CLIENT_SECRET: &str = "be30b9f4-6862-488a-99ad-21bde0400081";
pub(crate) const USER_AGENT: &str = "tqsdk-python 3.8.1";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const READ_TIMEOUT: Duration = Duration::from_secs(6);
const REQUEST_DEADLINE: Duration = Duration::from_secs(12);
const QUOTE_DEADLINE: Duration = Duration::from_secs(5);

pub const MISSING_KEY: &str = "Set up your TqSdk account in Settings before using futures charts.";

const FUTURES_EXCHANGES: [&str; 6] = ["SHFE", "DCE", "CZCE", "CFFEX", "INE", "GFEX"];

const INS_QUERY_ID: &str = "PYSDK_api_1";
const INS_GRAPHQL_QUERY: &str = "query{multi_symbol_info(class:[FUTURE,CONT,INDEX],expired:false){... on basic{class instrument_id instrument_name exchange_id price_tick price_decs py_wh english_name} ... on tradeable{volume_multiple} ... on future{product_id expire_datetime} ... on derivative{underlying{edges{node{... on basic{instrument_id}}}}}}}";

static CATALOG: Catalog<FuturesAsset> = Catalog::new("catalog_tqsdk");
static CHART_SEQ: AtomicU64 = AtomicU64::new(1);
static AUTH_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
});

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FuturesAsset {
    pub symbol: String,
    pub name: String,
    pub exchange_id: String,
    pub class: String,
    pub product_id: String,
    pub pinyin: String,
    pub english_name: String,
    pub price_tick: f64,
    pub volume_multiple: f64,
    pub underlying_symbol: Option<String>,
}

#[derive(Clone)]
struct TokenEntry {
    username: String,
    password: String,
    access_token: String,
    md_url: String,
    expires_at: Instant,
}

static TOKEN_CACHE: LazyLock<Mutex<Option<TokenEntry>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Deserialize)]
struct NsResponse {
    #[serde(default)]
    mdurl: String,
}

pub struct TqSdk;

pub static TQSDK: TqSdk = TqSdk;

pub fn clear_catalog() {
    CATALOG.clear_memory();
    *lock(&TOKEN_CACHE) = None;
}

/// Look up cached metadata (multiplier, tick size, underlying contract) for a futures symbol.
pub fn asset_meta(symbol: &str) -> Option<FuturesAsset> {
    let assets = CATALOG.get(fetch_catalog).ok()?;
    assets
        .iter()
        .find(|asset| asset.symbol == symbol || asset.symbol.eq_ignore_ascii_case(symbol))
        .cloned()
}

fn credentials() -> Result<(String, String), ProviderError> {
    tqsdk_credentials().ok_or(ProviderError::MissingTqSdkKey)
}

fn form_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

fn request_token(username: &str, password: &str) -> Result<(String, u64), ProviderError> {
    let body = format!(
        "grant_type=password&client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}&username={}&password={}",
        form_encode(username),
        form_encode(password),
    );
    let response = AUTH_AGENT
        .post(AUTH_URL)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body.as_bytes())
        .map_err(|error| ProviderError::TqSdk(net_http::Error::Network(error.to_string())))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(ProviderError::TqSdk(net_http::Error::Status(status)));
    }
    let body_text = response
        .into_body()
        .read_to_string()
        .map_err(ProviderError::data)?;
    let parsed: AuthResponse = serde_json::from_str(&body_text).map_err(ProviderError::data)?;
    if parsed.access_token.is_empty() {
        return Err(ProviderError::data("empty access_token from Shinny auth"));
    }
    Ok((parsed.access_token, parsed.expires_in.max(300)))
}

fn resolve_md_url(access_token: &str) -> String {
    let auth = format!("Bearer {access_token}");
    let Ok(response) = AUTH_AGENT
        .get(NS_URL)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .header("Authorization", &auth)
        .call()
    else {
        return DEFAULT_MD_URL.to_owned();
    };
    if response.status().as_u16() >= 400 {
        return DEFAULT_MD_URL.to_owned();
    }
    response
        .into_body()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str::<NsResponse>(&text).ok())
        .map(|ns| ns.mdurl.trim().to_owned())
        .filter(|url| url.starts_with("wss://") || url.starts_with("ws://"))
        .unwrap_or_else(|| DEFAULT_MD_URL.to_owned())
}

pub(crate) fn authenticate_session() -> Result<(String, String), ProviderError> {
    let (username, password) = credentials()?;
    {
        let cached = lock(&TOKEN_CACHE);
        if let Some(entry) = cached.as_ref()
            && entry.username == username
            && entry.password == password
            && Instant::now() < entry.expires_at
        {
            return Ok((entry.access_token.clone(), entry.md_url.clone()));
        }
    }
    let (access_token, expires_in) = request_token(&username, &password)?;
    let md_url = resolve_md_url(&access_token);
    let ttl = Duration::from_secs(expires_in.saturating_sub(60).max(60));
    *lock(&TOKEN_CACHE) = Some(TokenEntry {
        username,
        password,
        access_token: access_token.clone(),
        md_url: md_url.clone(),
        expires_at: Instant::now() + ttl,
    });
    Ok((access_token, md_url))
}

/// Checks `username` and `password` against Shinny's OAuth server. Used by Settings' Test button.
pub fn test_connection(username: &str, password: &str) -> Result<(), ProviderError> {
    let username = username.trim();
    let password = password.trim();
    if username.is_empty() || password.is_empty() {
        return Err(ProviderError::MissingTqSdkKey);
    }
    let _ = request_token(username, password)?;
    Ok(())
}

const MAX_WS_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Synchronous RFC 6455 WebSocket wrapper with RFC 7692 `permessage-deflate` decompression.
///
/// Shinny's DIFF gateways require `Sec-WebSocket-Extensions: permessage-deflate;
/// client_no_context_takeover; server_no_context_takeover` and always send compressed frames
/// (`RSV1=1`), which `tungstenite` 0.30 rejects with `Reserved bits are non-zero`.
pub(crate) struct TqWebSocket {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    deflate: bool,
}

impl TqWebSocket {
    pub(crate) fn set_read_timeout(&mut self, timeout: Duration) {
        match self.socket.get_mut() {
            MaybeTlsStream::Plain(stream) => {
                let _ = stream.set_read_timeout(Some(timeout));
            }
            MaybeTlsStream::Rustls(tls) => {
                let _ = tls.sock.set_read_timeout(Some(timeout));
            }
            _ => {}
        }
    }

    pub(crate) fn send_text(&mut self, text: &str) -> io::Result<()> {
        write_ws_frame(self.socket.get_mut(), 0x81, text.as_bytes())
    }

    pub(crate) fn close(&mut self) {
        let _ = write_ws_frame(self.socket.get_mut(), 0x88, &[]);
    }

    pub(crate) fn read_text(&mut self) -> io::Result<String> {
        let mut msg_buf = Vec::new();
        let mut msg_compressed = false;
        let mut in_fragment = false;

        loop {
            let (fin, rsv1, opcode, payload) = read_ws_frame(self.socket.get_mut())?;
            match opcode {
                0x8 => {
                    let close_info = if payload.len() >= 2 {
                        let code = u16::from_be_bytes([payload[0], payload[1]]);
                        let reason = String::from_utf8_lossy(&payload[2..]);
                        format!("TqSdk WebSocket closed by server ({code}: {reason})")
                    } else {
                        "TqSdk WebSocket closed by server".to_owned()
                    };
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, close_info));
                }
                0x9 => {
                    write_ws_frame(self.socket.get_mut(), 0x8A, &payload)?;
                }
                0xA => {}
                0x1 | 0x2 => {
                    msg_buf.clear();
                    msg_buf.extend_from_slice(&payload);
                    msg_compressed = rsv1 && self.deflate;
                    in_fragment = !fin;
                    if fin {
                        break;
                    }
                }
                0x0 if in_fragment => {
                    if msg_buf.len().saturating_add(payload.len()) > MAX_WS_FRAME_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "TqSdk WebSocket message exceeded 64 MiB limit",
                        ));
                    }
                    msg_buf.extend_from_slice(&payload);
                    if fin {
                        break;
                    }
                }
                _ => {}
            }
        }

        let bytes = if msg_compressed {
            inflate_permessage(&mut msg_buf)?
        } else {
            msg_buf
        };
        String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

fn read_ws_frame<R: Read>(stream: &mut R) -> io::Result<(bool, bool, u8, Vec<u8>)> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header)?;
    let fin = (header[0] & 0x80) != 0;
    let rsv1 = (header[0] & 0x40) != 0;
    let opcode = header[0] & 0x0F;
    let masked = (header[1] & 0x80) != 0;
    let len7 = header[1] & 0x7F;

    let payload_len: usize = match len7 {
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext)?;
            u16::from_be_bytes(ext) as usize
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext)?;
            usize::try_from(u64::from_be_bytes(ext)).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "WebSocket frame length overflow",
                )
            })?
        }
        n => n as usize,
    };

    if payload_len > MAX_WS_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "TqSdk WebSocket frame exceeded 64 MiB limit",
        ));
    }

    let mut mask_key = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask_key)?;
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream.read_exact(&mut payload)?;
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask_key[i & 3];
            }
        }
    }

    Ok((fin, rsv1, opcode, payload))
}

fn write_ws_frame<W: Write>(stream: &mut W, first_byte: u8, payload: &[u8]) -> io::Result<()> {
    let mut mask = [0u8; 4];
    getrandom::fill(&mut mask)
        .map_err(|error| io::Error::other(format!("mask RNG failed: {error}")))?;

    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.push(first_byte);
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | (len as u8));
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    for (i, &byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[i & 3]);
    }
    stream.write_all(&frame)?;
    stream.flush()
}

fn inflate_permessage(compressed: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    // RFC 7692 §7.2.2: append 0x00 0x00 0xFF 0xFF tail before raw deflate decompression.
    compressed.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
    let mut decompressor = Decompress::new(false);
    let mut out = Vec::with_capacity(compressed.len().saturating_mul(4).max(256));
    let mut in_offset = 0usize;

    loop {
        if out.len() == out.capacity() {
            out.reserve(out.capacity().max(1024));
        }
        let prev_in = decompressor.total_in();
        let prev_out = decompressor.total_out();
        let spare = out.spare_capacity_mut();
        // Safety: only the bytes written by `decompress` (`delta_out`) are exposed via `set_len`.
        let status = unsafe {
            let slice =
                std::slice::from_raw_parts_mut(spare.as_mut_ptr().cast::<u8>(), spare.len());
            decompressor.decompress(&compressed[in_offset..], slice, FlushDecompress::Sync)
        }
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let delta_in = (decompressor.total_in() - prev_in) as usize;
        let delta_out = (decompressor.total_out() - prev_out) as usize;
        in_offset += delta_in;
        unsafe {
            out.set_len(out.len() + delta_out);
        }

        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                if in_offset >= compressed.len() && delta_out == 0 {
                    break;
                }
                if delta_in == 0 && delta_out == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "permessage-deflate decompression stalled",
                    ));
                }
            }
        }
    }

    Ok(out)
}

/// Opens an authenticated DIFF WebSocket to the TqSdk market-data gateway.
pub(crate) fn connect_md_socket() -> Result<TqWebSocket, ProviderError> {
    let (token, md_url) = authenticate_session()?;
    let auth = format!("Bearer {token}");
    connect_ws_with_headers(
        &md_url,
        &[
            ("User-Agent", USER_AGENT),
            ("Accept", "application/json"),
            ("Authorization", &auth),
            (
                "Sec-WebSocket-Extensions",
                "permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            ),
        ],
    )
    .map_err(ProviderError::other)
}

pub(crate) fn connect_ws_with_headers(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<TqWebSocket, String> {
    let mut request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    let host = request
        .uri()
        .host()
        .ok_or_else(|| "missing host in WebSocket URL".to_owned())?
        .to_owned();
    let port = request.uri().port_u16().unwrap_or_else(|| {
        if request.uri().scheme_str() == Some("ws") {
            80
        } else {
            443
        }
    });
    for (name, value) in headers {
        let header_name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|error| error.to_string())?;
        let header_value = HeaderValue::from_str(value).map_err(|error| error.to_string())?;
        request.headers_mut().insert(header_name, header_value);
    }

    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?;
    let mut last_error = None;
    for address in addresses.take(4) {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(READ_TIMEOUT))
                    .map_err(|error| error.to_string())?;
                stream
                    .set_write_timeout(Some(READ_TIMEOUT))
                    .map_err(|error| error.to_string())?;
                let (socket, response) =
                    tungstenite::client_tls(request, stream).map_err(|error| {
                        if let tungstenite::HandshakeError::Failure(tungstenite::Error::Http(
                            resp,
                        )) = &error
                        {
                            let body = resp
                                .body()
                                .as_deref()
                                .map(|b| String::from_utf8_lossy(b).into_owned())
                                .unwrap_or_default();
                            format!(
                                "{} (url={url}, headers={:?}, body={body})",
                                error,
                                resp.headers()
                            )
                        } else {
                            format!("{error} (url={url})")
                        }
                    })?;
                let deflate = response
                    .headers()
                    .get("sec-websocket-extensions")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("permessage-deflate"));
                return Ok(TqWebSocket { socket, deflate });
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.map_or_else(
        || "no address for TqSdk WebSocket host".to_owned(),
        |error| error.to_string(),
    ))
}

/// Merges a TqSdk DIFF patch object into `target` in place (null values delete keys).
pub(crate) fn merge_diff(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                if value.is_null() {
                    target_map.remove(&key);
                } else if let Some(existing) = target_map.get_mut(&key) {
                    if existing.is_object() && value.is_object() {
                        merge_diff(existing, value);
                    } else {
                        *existing = value;
                    }
                } else {
                    target_map.insert(key, value);
                }
            }
        }
        (target_slot, patch_value) => {
            *target_slot = patch_value;
        }
    }
}

fn send_json(socket: &mut TqWebSocket, payload: &Value) -> Result<(), ProviderError> {
    socket
        .send_text(&payload.to_string())
        .map_err(|error| ProviderError::other(format!("TqSdk WebSocket send failed: {error}")))
}

fn peek_message(socket: &mut TqWebSocket) -> Result<(), ProviderError> {
    send_json(socket, &json!({ "aid": "peek_message" }))
}

/// Reads the next DIFF `rtn_data` pack and applies its `data` array to `state`.
/// Returns `Ok(true)` when a `rtn_data` pack was merged, `Ok(false)` on another frame.
pub(crate) fn read_and_merge_pack(
    socket: &mut TqWebSocket,
    state: &mut Value,
) -> Result<bool, ProviderError> {
    let text = socket
        .read_text()
        .map_err(|error| ProviderError::other(format!("TqSdk WebSocket read failed: {error}")))?;
    let pack: Value = serde_json::from_str(&text).map_err(ProviderError::data)?;
    if pack.get("aid").and_then(Value::as_str) == Some("rtn_data") {
        if let Some(items) = pack.get("data").and_then(Value::as_array) {
            for diff in items {
                merge_diff(state, diff.clone());
            }
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn fetch_catalog() -> Result<Vec<FuturesAsset>, ProviderError> {
    if let Ok((_, md_url)) = authenticate_session()
        && !md_url.contains("free-openmd")
        && let Ok(assets) = fetch_catalog_ins_query()
        && !assets.is_empty()
    {
        return Ok(assets);
    }
    Ok(builtin_futures_catalog())
}

fn fetch_catalog_ins_query() -> Result<Vec<FuturesAsset>, ProviderError> {
    let mut socket = connect_md_socket()?;
    send_json(
        &mut socket,
        &json!({
            "aid": "ins_query",
            "query_id": INS_QUERY_ID,
            "query": INS_GRAPHQL_QUERY,
        }),
    )?;
    peek_message(&mut socket)?;

    let mut state = json!({});
    let start = Instant::now();
    while start.elapsed() < REQUEST_DEADLINE {
        let merged = read_and_merge_pack(&mut socket, &mut state)?;
        if merged {
            if let Some(list) = state
                .pointer(&format!("/symbols/{INS_QUERY_ID}/result/multi_symbol_info"))
                .and_then(Value::as_array)
            {
                let assets = parse_catalog_nodes(list);
                let _ = send_json(
                    &mut socket,
                    &json!({
                        "aid": "ins_query",
                        "query_id": INS_QUERY_ID,
                        "query": ""
                    }),
                );
                socket.close();
                return Ok(assets);
            }
            peek_message(&mut socket)?;
        }
    }
    Err(ProviderError::other(
        "Timed out loading futures contract catalog from TqSdk",
    ))
}

struct BuiltinProduct {
    exchange: &'static str,
    product: &'static str,
    name: &'static str,
    pinyin: &'static str,
    english: &'static str,
    price_tick: f64,
    volume_multiple: f64,
}

const BUILTIN_PRODUCTS: &[BuiltinProduct] = &[
    // SHFE
    BuiltinProduct {
        exchange: "SHFE",
        product: "rb",
        name: "螺纹钢",
        pinyin: "lwg,luoweng",
        english: "Rebar",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "hc",
        name: "热轧卷板",
        pinyin: "rj,rzjb,rejuan",
        english: "Hot Rolled Coil",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "au",
        name: "沪金",
        pinyin: "hj,hujin,gold",
        english: "Gold",
        price_tick: 0.02,
        volume_multiple: 1000.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "ag",
        name: "沪银",
        pinyin: "hy,huyin,silver",
        english: "Silver",
        price_tick: 1.0,
        volume_multiple: 15.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "cu",
        name: "沪铜",
        pinyin: "ht,hutong,copper",
        english: "Copper",
        price_tick: 10.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "al",
        name: "沪铝",
        pinyin: "hl,hulv,aluminum",
        english: "Aluminum",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "zn",
        name: "沪锌",
        pinyin: "hx,huxin,zinc",
        english: "Zinc",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "pb",
        name: "沪铅",
        pinyin: "hq,huqian,lead",
        english: "Lead",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "ni",
        name: "沪镍",
        pinyin: "hn,hunie,nickel",
        english: "Nickel",
        price_tick: 10.0,
        volume_multiple: 1.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "sn",
        name: "沪锡",
        pinyin: "hx,huxi,tin",
        english: "Tin",
        price_tick: 10.0,
        volume_multiple: 1.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "ss",
        name: "不锈钢",
        pinyin: "bxg,buxiugang",
        english: "Stainless Steel",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "fu",
        name: "燃油",
        pinyin: "ry,ranyou",
        english: "Fuel Oil",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "bu",
        name: "沥青",
        pinyin: "lq,liqing",
        english: "Bitumen",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "ru",
        name: "天然橡胶",
        pinyin: "xj,trxj,xiangjiao",
        english: "Rubber",
        price_tick: 5.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "sp",
        name: "纸浆",
        pinyin: "zj,zhijiang",
        english: "Pulp",
        price_tick: 2.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "ao",
        name: "氧化铝",
        pinyin: "yhl,yanghualv",
        english: "Alumina",
        price_tick: 1.0,
        volume_multiple: 20.0,
    },
    BuiltinProduct {
        exchange: "SHFE",
        product: "br",
        name: "丁二烯橡胶",
        pinyin: "dexxj,br",
        english: "Butadiene Rubber",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    // INE
    BuiltinProduct {
        exchange: "INE",
        product: "sc",
        name: "原油",
        pinyin: "yy,yuanyou,crude",
        english: "Crude Oil",
        price_tick: 0.1,
        volume_multiple: 1000.0,
    },
    BuiltinProduct {
        exchange: "INE",
        product: "lu",
        name: "低硫燃料油",
        pinyin: "dlry,diliu",
        english: "Low Sulfur Fuel Oil",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "INE",
        product: "nr",
        name: "20号胶",
        pinyin: "nr,20haojiao",
        english: "TSR 20",
        price_tick: 5.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "INE",
        product: "bc",
        name: "国际铜",
        pinyin: "gjt,guojitong",
        english: "Bonded Copper",
        price_tick: 10.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "INE",
        product: "ec",
        name: "集运欧线",
        pinyin: "jyox,jiyun",
        english: "Containerized Freight",
        price_tick: 0.1,
        volume_multiple: 50.0,
    },
    // CFFEX
    BuiltinProduct {
        exchange: "CFFEX",
        product: "IF",
        name: "沪深300",
        pinyin: "hs,hushen300",
        english: "CSI 300",
        price_tick: 0.2,
        volume_multiple: 300.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "IH",
        name: "上证50",
        pinyin: "sz,shangzheng50",
        english: "SSE 50",
        price_tick: 0.2,
        volume_multiple: 300.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "IC",
        name: "中证500",
        pinyin: "zz500,zhongzheng500",
        english: "CSI 500",
        price_tick: 0.2,
        volume_multiple: 200.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "IM",
        name: "中证1000",
        pinyin: "zz1000,zhongzheng1000",
        english: "CSI 1000",
        price_tick: 0.2,
        volume_multiple: 200.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "T",
        name: "十债",
        pinyin: "sz,shizhai",
        english: "10Y Treasury",
        price_tick: 0.005,
        volume_multiple: 10000.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "TF",
        name: "五债",
        pinyin: "wz,wuzhai",
        english: "5Y Treasury",
        price_tick: 0.005,
        volume_multiple: 10000.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "TS",
        name: "二债",
        pinyin: "ez,erzhai",
        english: "2Y Treasury",
        price_tick: 0.002,
        volume_multiple: 20000.0,
    },
    BuiltinProduct {
        exchange: "CFFEX",
        product: "TL",
        name: "三十债",
        pinyin: "ssz,sanshizhai",
        english: "30Y Treasury",
        price_tick: 0.01,
        volume_multiple: 10000.0,
    },
    // DCE
    BuiltinProduct {
        exchange: "DCE",
        product: "i",
        name: "铁矿石",
        pinyin: "tks,tiekuangshi",
        english: "Iron Ore",
        price_tick: 0.5,
        volume_multiple: 100.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "j",
        name: "焦炭",
        pinyin: "jt,jiaotan",
        english: "Coke",
        price_tick: 0.5,
        volume_multiple: 100.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "jm",
        name: "焦煤",
        pinyin: "jm,jiaomei",
        english: "Coking Coal",
        price_tick: 0.5,
        volume_multiple: 60.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "m",
        name: "豆粕",
        pinyin: "dp,doupo",
        english: "Soybean Meal",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "y",
        name: "豆油",
        pinyin: "dy,douyou",
        english: "Soybean Oil",
        price_tick: 2.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "a",
        name: "豆一",
        pinyin: "dy,douyi",
        english: "No.1 Soybean",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "b",
        name: "豆二",
        pinyin: "de,douer",
        english: "No.2 Soybean",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "p",
        name: "棕榈油",
        pinyin: "zly,zonglvyou",
        english: "Palm Oil",
        price_tick: 2.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "c",
        name: "玉米",
        pinyin: "ym,yumi",
        english: "Corn",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "cs",
        name: "玉米淀粉",
        pinyin: "df,dianfen",
        english: "Corn Starch",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "jd",
        name: "鸡蛋",
        pinyin: "jd,jidan",
        english: "Egg",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "lh",
        name: "生猪",
        pinyin: "sz,shengzhu",
        english: "Live Hog",
        price_tick: 5.0,
        volume_multiple: 16.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "l",
        name: "塑料",
        pinyin: "sl,suliao",
        english: "LLDPE",
        price_tick: 1.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "v",
        name: "PVC",
        pinyin: "pvc",
        english: "PVC",
        price_tick: 1.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "pp",
        name: "聚丙烯",
        pinyin: "jbx,jubingxi",
        english: "Polypropylene",
        price_tick: 1.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "eg",
        name: "乙二醇",
        pinyin: "yec,yierchun",
        english: "MEG",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "eb",
        name: "苯乙烯",
        pinyin: "byx,benyixi",
        english: "Styrene",
        price_tick: 1.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "DCE",
        product: "pg",
        name: "液化石油气",
        pinyin: "lpg,yhq",
        english: "LPG",
        price_tick: 1.0,
        volume_multiple: 20.0,
    },
    // CZCE
    BuiltinProduct {
        exchange: "CZCE",
        product: "SR",
        name: "白糖",
        pinyin: "bt,baitang",
        english: "Sugar",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "CF",
        name: "棉花",
        pinyin: "mh,mianhua",
        english: "Cotton",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "TA",
        name: "PTA",
        pinyin: "pta",
        english: "PTA",
        price_tick: 2.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "MA",
        name: "甲醇",
        pinyin: "jc,jiachun",
        english: "Methanol",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "FG",
        name: "玻璃",
        pinyin: "bl,boli",
        english: "Glass",
        price_tick: 1.0,
        volume_multiple: 20.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "SA",
        name: "纯碱",
        pinyin: "cj,chunjian",
        english: "Soda Ash",
        price_tick: 1.0,
        volume_multiple: 20.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "OI",
        name: "菜油",
        pinyin: "cy,caiyou",
        english: "Rapeseed Oil",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "RM",
        name: "菜粕",
        pinyin: "cp,caipo",
        english: "Rapeseed Meal",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "SF",
        name: "硅铁",
        pinyin: "gt,guitie",
        english: "Ferrosilicon",
        price_tick: 2.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "SM",
        name: "锰硅",
        pinyin: "mg,menggui",
        english: "Manganese Silicon",
        price_tick: 2.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "AP",
        name: "苹果",
        pinyin: "pg,pingguo",
        english: "Apple",
        price_tick: 1.0,
        volume_multiple: 10.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "UR",
        name: "尿素",
        pinyin: "ns,niaosu",
        english: "Urea",
        price_tick: 1.0,
        volume_multiple: 20.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "PF",
        name: "短纤",
        pinyin: "dx,duanxian",
        english: "Staple Fiber",
        price_tick: 2.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "PX",
        name: "对二甲苯",
        pinyin: "px,dejb",
        english: "Paraxylene",
        price_tick: 2.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "CZCE",
        product: "SH",
        name: "烧碱",
        pinyin: "sj,shaojian",
        english: "Caustic Soda",
        price_tick: 1.0,
        volume_multiple: 30.0,
    },
    // GFEX
    BuiltinProduct {
        exchange: "GFEX",
        product: "si",
        name: "工业硅",
        pinyin: "gyg,gongyegui",
        english: "Industrial Silicon",
        price_tick: 5.0,
        volume_multiple: 5.0,
    },
    BuiltinProduct {
        exchange: "GFEX",
        product: "lc",
        name: "碳酸锂",
        pinyin: "tsl,tansuanli",
        english: "Lithium Carbonate",
        price_tick: 20.0,
        volume_multiple: 1.0,
    },
    BuiltinProduct {
        exchange: "GFEX",
        product: "ps",
        name: "多晶硅",
        pinyin: "djg,duojinggui",
        english: "Polysilicon",
        price_tick: 5.0,
        volume_multiple: 3.0,
    },
];

fn builtin_futures_catalog() -> Vec<FuturesAsset> {
    let mut assets = Vec::with_capacity(BUILTIN_PRODUCTS.len() * 2);
    for prod in BUILTIN_PRODUCTS {
        assets.push(FuturesAsset {
            symbol: format!("KQ.m@{}.{}", prod.exchange, prod.product),
            name: format!("{}主连", prod.name),
            exchange_id: "KQ".to_owned(),
            class: "CONT".to_owned(),
            product_id: prod.product.to_owned(),
            pinyin: format!("{},{}zl", prod.pinyin, prod.product.to_lowercase()),
            english_name: format!("{} Main", prod.english),
            price_tick: prod.price_tick,
            volume_multiple: prod.volume_multiple,
            underlying_symbol: None,
        });
        assets.push(FuturesAsset {
            symbol: format!("KQ.i@{}.{}", prod.exchange, prod.product),
            name: format!("{}指数", prod.name),
            exchange_id: "KQ".to_owned(),
            class: "INDEX".to_owned(),
            product_id: prod.product.to_owned(),
            pinyin: format!("{},{}zs", prod.pinyin, prod.product.to_lowercase()),
            english_name: format!("{} Index", prod.english),
            price_tick: prod.price_tick,
            volume_multiple: prod.volume_multiple,
            underlying_symbol: None,
        });
    }
    assets
}

fn parse_catalog_nodes(nodes: &[Value]) -> Vec<FuturesAsset> {
    let mut assets = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.get("expired").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let raw_class = node
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let class = match raw_class {
            "FUTURE_CONT" => "CONT",
            "FUTURE_INDEX" => "INDEX",
            other => other,
        };
        let exchange_id = node
            .get("exchange_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let symbol = node
            .get("instrument_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if symbol.is_empty() || !is_supported_futures_node(class, exchange_id, symbol) {
            continue;
        }
        let name = node
            .get("ins_name")
            .or_else(|| node.get("instrument_name"))
            .and_then(Value::as_str)
            .unwrap_or(symbol)
            .to_owned();
        let pinyin = node
            .get("py_wh")
            .or_else(|| node.get("py"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let english_name = node
            .get("english_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let price_tick = node
            .get("price_tick")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.01);
        let volume_multiple = node
            .get("volume_multiple")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0);
        let underlying_symbol = node
            .get("underlying_symbol")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                node.pointer("/underlying/edges/0/node/instrument_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            })
            .map(str::to_owned);
        let product_id = node
            .get("product_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| infer_product_id(symbol));

        assets.push(FuturesAsset {
            symbol: symbol.to_owned(),
            name,
            exchange_id: exchange_id.to_owned(),
            class: class.to_owned(),
            product_id,
            pinyin,
            english_name,
            price_tick,
            volume_multiple,
            underlying_symbol,
        });
    }
    assets.sort_by(|a, b| {
        class_priority(&a.class)
            .cmp(&class_priority(&b.class))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    assets
}

fn is_supported_futures_node(class: &str, exchange_id: &str, symbol: &str) -> bool {
    match class {
        "FUTURE" => FUTURES_EXCHANGES.contains(&exchange_id),
        "CONT" | "INDEX" => {
            exchange_id == "KQ"
                && FUTURES_EXCHANGES
                    .iter()
                    .any(|ex| symbol.contains(&format!("@{ex}.")))
        }
        _ => false,
    }
}

fn infer_product_id(symbol: &str) -> String {
    let rest = symbol
        .strip_prefix("KQ.m@")
        .or_else(|| symbol.strip_prefix("KQ.i@"))
        .unwrap_or(symbol);
    let after_dot = rest.split_once('.').map_or(rest, |(_, code)| code);
    after_dot
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_owned()
}

fn class_priority(class: &str) -> u8 {
    match class {
        "CONT" => 0,
        "FUTURE" => 1,
        "INDEX" => 2,
        _ => 3,
    }
}

const DEFAULT_POPULAR_SYMBOLS: [&str; 12] = [
    "KQ.m@SHFE.rb",
    "KQ.m@SHFE.au",
    "KQ.m@SHFE.ag",
    "KQ.m@SHFE.cu",
    "KQ.m@CFFEX.IF",
    "KQ.m@CFFEX.IC",
    "KQ.m@CFFEX.IM",
    "KQ.m@DCE.m",
    "KQ.m@DCE.i",
    "KQ.m@CZCE.TA",
    "KQ.m@CZCE.MA",
    "KQ.m@INE.sc",
];

fn search_catalog(assets: &[FuturesAsset], query: &str) -> Vec<SearchResult> {
    let raw = query.trim();
    if raw.is_empty() {
        let mut popular = Vec::new();
        for symbol in DEFAULT_POPULAR_SYMBOLS {
            if let Some(asset) = assets.iter().find(|a| a.symbol == symbol) {
                popular.push(to_search_result(asset));
            }
        }
        if popular.is_empty() {
            popular.extend(assets.iter().take(20).map(to_search_result));
        }
        return popular;
    }

    let lower = raw.to_lowercase();
    let mut ranked: Vec<(u8, u8, &FuturesAsset)> = assets
        .iter()
        .filter_map(|asset| {
            let tier = rank_asset(asset, raw, &lower)?;
            Some((tier, class_priority(&asset.class), asset))
        })
        .collect();

    ranked.sort_by(|(tier_a, class_a, a), (tier_b, class_b, b)| {
        tier_a
            .cmp(tier_b)
            .then_with(|| class_a.cmp(class_b))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });

    let mut out: Vec<SearchResult> = ranked
        .into_iter()
        .take(40)
        .map(|(_, _, asset)| to_search_result(asset))
        .collect();

    if let Some(direct) = synthesize_direct_contract(assets, raw)
        && !out.iter().any(|r| r.full_symbol == direct.full_symbol)
    {
        out.insert(0, direct);
    }
    out
}

fn synthesize_direct_contract(assets: &[FuturesAsset], raw: &str) -> Option<SearchResult> {
    let (explicit_ex, code) = raw
        .split_once('.')
        .map_or((None, raw), |(ex, c)| (Some(ex), c));
    let digit_pos = code.find(|c: char| c.is_ascii_digit())?;
    let (prod_part, month_part) = code.split_at(digit_pos);
    if prod_part.is_empty()
        || !(3..=4).contains(&month_part.len())
        || !month_part.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let template = assets.iter().find(|a| {
        a.product_id.eq_ignore_ascii_case(prod_part)
            && explicit_ex.is_none_or(|ex| {
                a.exchange_id.eq_ignore_ascii_case(ex)
                    || a.symbol.contains(&format!("@{}.", ex.to_ascii_uppercase()))
            })
    })?;
    let exchange = if let Some(ex) = explicit_ex {
        ex.to_ascii_uppercase()
    } else if let Some((_, rest)) = template.symbol.split_once('@') {
        rest.split_once('.').map(|(ex, _)| ex.to_owned())?
    } else {
        template.exchange_id.clone()
    };
    let full_symbol = format!("{exchange}.{}{month_part}", template.product_id);
    let base_name = template
        .name
        .trim_end_matches("主连")
        .trim_end_matches("指数");
    let display = format!("{full_symbol}{NAME_SEPARATOR}{base_name}{month_part}");
    Some(SearchResult::new(DataSource::TqSdk, display, full_symbol))
}

fn to_search_result(asset: &FuturesAsset) -> SearchResult {
    let display = if asset.name.is_empty() {
        asset.symbol.clone()
    } else {
        format!("{}{NAME_SEPARATOR}{}", asset.symbol, asset.name)
    };
    SearchResult::new(DataSource::TqSdk, display, asset.symbol.clone())
}

fn rank_asset(asset: &FuturesAsset, raw: &str, lower: &str) -> Option<u8> {
    let sym_lower = asset.symbol.to_lowercase();
    let code_after_dot = sym_lower
        .rsplit_once('.')
        .map_or(sym_lower.as_str(), |(_, c)| c);
    let prod_lower = asset.product_id.to_lowercase();
    let py_lower = asset.pinyin.to_lowercase();
    let en_lower = asset.english_name.to_lowercase();

    if sym_lower == lower
        || code_after_dot == lower
        || asset.name == raw
        || (prod_lower == lower && asset.class == "CONT")
    {
        return Some(0);
    }
    if prod_lower == lower || py_lower == lower {
        return Some(1);
    }
    if code_after_dot.starts_with(lower)
        || sym_lower.starts_with(lower)
        || py_lower.starts_with(lower)
        || asset.name.starts_with(raw)
    {
        return Some(2);
    }
    if sym_lower.contains(lower)
        || asset.name.contains(raw)
        || py_lower.contains(lower)
        || (!en_lower.is_empty() && en_lower.contains(lower))
    {
        return Some(3);
    }
    None
}

/// Converts a DegenView interval string into `(native_duration_secs, calendar_fold_target_secs, days_per_bar)`.
fn interval_plan(interval: &str) -> Result<(i64, Option<(i64, u32)>), ProviderError> {
    match interval {
        "1m" => Ok((60, None)),
        "3m" => Ok((180, None)),
        "5m" => Ok((300, None)),
        "15m" => Ok((900, None)),
        "30m" => Ok((1_800, None)),
        "1h" => Ok((3_600, None)),
        "4h" => Ok((14_400, None)),
        "1d" => Ok((86_400, None)),
        "1w" => Ok((86_400, Some((7 * 86_400, 7)))),
        "1M" => Ok((86_400, Some((30 * 86_400, 32)))),
        "3M" => Ok((86_400, Some((90 * 86_400, 95)))),
        "1Y" => Ok((86_400, Some((365 * 86_400, 370)))),
        other => Err(ProviderError::other(format!(
            "Unsupported TqSdk interval: {other}"
        ))),
    }
}

/// Native duration in nanoseconds for live streaming (`1m`..=`1d`).
pub(crate) fn live_duration_ns(interval: &str) -> Option<i64> {
    let (secs, fold) = interval_plan(interval).ok()?;
    fold.is_none().then_some(secs * 1_000_000_000)
}

fn fetch_chart_klines(
    symbol: &str,
    duration_secs: i64,
    view_width: u32,
) -> Result<Vec<Kline>, ProviderError> {
    let mut socket = connect_md_socket()?;
    let duration_ns = duration_secs * 1_000_000_000;
    let width = view_width.clamp(1, 10_000) as i64;
    let chart_id = format!("DGV_chart_{}", CHART_SEQ.fetch_add(1, Ordering::Relaxed));

    send_json(
        &mut socket,
        &json!({
            "aid": "set_chart",
            "chart_id": chart_id,
            "ins_list": symbol,
            "duration": duration_ns,
            "view_width": width,
        }),
    )?;
    peek_message(&mut socket)?;

    let mut state = json!({});
    let start = Instant::now();
    let duration_key = duration_ns.to_string();

    while start.elapsed() < REQUEST_DEADLINE {
        let merged = match read_and_merge_pack(&mut socket, &mut state) {
            Ok(m) => m,
            Err(err) => {
                let fallback = extract_klines(&state, symbol, &duration_key, duration_secs * 1_000);
                if !fallback.is_empty() {
                    return Ok(fallback);
                }
                return Err(err);
            }
        };
        if !merged {
            continue;
        }
        let more_data = state
            .get("mdhis_more_data")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let chart = state.pointer(&format!("/charts/{chart_id}"));
        let chart_more_data = chart
            .and_then(|c| c.get("more_data"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state_ready = chart.and_then(|c| c.get("state")).is_some_and(|s| {
            s.get("ins_list").and_then(Value::as_str) == Some(symbol)
                && s.get("duration").and_then(Value::as_i64) == Some(duration_ns)
        });
        let left_id = chart.and_then(|c| c.get("left_id")).and_then(Value::as_i64);
        let right_id = chart
            .and_then(|c| c.get("right_id"))
            .and_then(Value::as_i64);

        if state_ready
            && !more_data
            && !chart_more_data
            && let (Some(left), Some(right)) = (left_id, right_id)
            && left >= 0
            && right >= left
        {
            let expected = (right - left + 1) as usize;
            let klines = extract_klines(&state, symbol, &duration_key, duration_secs * 1_000);
            let full_window = left == 0 || expected >= width as usize;
            if !klines.is_empty() && full_window {
                let _ = send_json(
                    &mut socket,
                    &json!({
                        "aid": "set_chart",
                        "chart_id": chart_id,
                        "ins_list": "",
                        "duration": duration_ns,
                        "view_width": width,
                    }),
                );
                socket.close();
                return Ok(klines);
            }
        }
        peek_message(&mut socket)?;
    }

    let fallback = extract_klines(&state, symbol, &duration_key, duration_secs * 1_000);
    if !fallback.is_empty() {
        return Ok(fallback);
    }
    Err(ProviderError::other(format!(
        "Timed out fetching TqSdk klines for {symbol}"
    )))
}

pub(crate) fn extract_klines(
    state: &Value,
    symbol: &str,
    duration_key: &str,
    duration_ms: i64,
) -> Vec<Kline> {
    let Some(data_map) = state
        .get("klines")
        .and_then(|k| k.get(symbol))
        .and_then(|s| s.get(duration_key))
        .and_then(|d| d.get("data"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut bars: Vec<(i64, Kline)> = data_map
        .iter()
        .filter_map(|(id_str, bar)| {
            let id = id_str.parse::<i64>().ok()?;
            let kline = parse_kline_entry(bar, duration_ms)?;
            Some((id, kline))
        })
        .collect();
    bars.sort_by_key(|(id, kline)| (*id, kline.open_time));
    bars.into_iter().map(|(_, kline)| kline).collect()
}

pub(crate) fn parse_kline_entry(bar: &Value, duration_ms: i64) -> Option<Kline> {
    let datetime_ns = bar.get("datetime")?.as_i64()?;
    if datetime_ns <= 0 {
        return None;
    }
    let open = bar.get("open")?.as_f64()?;
    let high = bar.get("high")?.as_f64()?;
    let low = bar.get("low")?.as_f64()?;
    let close = bar.get("close")?.as_f64()?;
    if !(open.is_finite() && high.is_finite() && low.is_finite() && close.is_finite()) {
        return None;
    }
    let volume = bar
        .get("volume")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0);
    let open_time = datetime_ns / 1_000_000;
    let close_time = open_time + duration_ms - 1;
    Some(Kline {
        open_time,
        open,
        high,
        low,
        close,
        volume,
        close_time,
    })
}

impl MarketSource for TqSdk {
    fn search_tickers(&self, query: &str) -> Result<Vec<SearchResult>, ProviderError> {
        let _ = credentials()?;
        let assets = CATALOG.get(fetch_catalog)?;
        Ok(search_catalog(&assets, query))
    }

    fn fetch_klines(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        limit: u32,
    ) -> Result<Vec<Kline>, ProviderError> {
        let symbol = symbol.trim();
        let plan = plan(DataSource::TqSdk, timeframe)?;
        let (duration_secs, fold) = interval_plan(&plan.base().interval())?;
        let bars = match fold {
            None => fetch_chart_klines(symbol, duration_secs, limit)?,
            Some((_target_secs, _days_per_bar)) => {
                let daily_limit = limit
                    .saturating_mul(_days_per_bar)
                    .saturating_add(30)
                    .clamp(300, 10_000);
                let daily = fetch_chart_klines(symbol, duration_secs, daily_limit)?;
                plan.fold(daily, false)
            }
        };
        Ok(bars)
    }
}

/// Fetches latest quotes for `DataSource::TqSdk` watchlist items via `subscribe_quote`.
pub fn quotes(requests: &[QuoteRequest]) -> Result<HashMap<String, SourceQuote>, QuoteFailure> {
    if requests.is_empty() {
        return Ok(HashMap::new());
    }
    if tqsdk_credentials().is_none() {
        return Err(QuoteFailure::TqSdkCredentialsMissing);
    }
    let mut socket =
        connect_md_socket().map_err(|error| QuoteFailure::Failed(error.to_string()))?;
    let symbols: Vec<&str> = requests.iter().map(|r| r.symbol.as_str()).collect();
    let ins_list = symbols.join(",");

    send_json(
        &mut socket,
        &json!({
            "aid": "subscribe_quote",
            "ins_list": ins_list,
        }),
    )
    .map_err(|error| QuoteFailure::Failed(error.to_string()))?;
    peek_message(&mut socket).map_err(|error| QuoteFailure::Failed(error.to_string()))?;

    let mut state = json!({});
    let start = Instant::now();
    let mut out = HashMap::new();

    while start.elapsed() < QUOTE_DEADLINE {
        let merged = read_and_merge_pack(&mut socket, &mut state)
            .map_err(|error| QuoteFailure::Failed(error.to_string()))?;
        if !merged {
            continue;
        }
        if let Some(quotes_obj) = state.get("quotes").and_then(Value::as_object) {
            for request in requests {
                if let Some(q) = quotes_obj.get(&request.symbol)
                    && let Some(parsed) = parse_quote_entry(q)
                {
                    out.insert(request.symbol.clone(), parsed);
                }
            }
        }
        if out.len() >= requests.len() {
            break;
        }
        let _ = peek_message(&mut socket);
    }
    socket.close();
    Ok(out)
}

fn finite_positive(val: Option<&Value>) -> Option<f64> {
    val.and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn parse_quote_entry(quote: &Value) -> Option<SourceQuote> {
    let previous = finite_positive(quote.get("pre_settlement"))
        .or_else(|| finite_positive(quote.get("pre_close")));
    let price = finite_positive(quote.get("last_price")).or(previous)?;
    let volume = quote
        .get("volume")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0);
    Some(SourceQuote {
        price,
        previous_day_price: previous,
        volume_24h: volume,
        volume_kind: Some(VolumeKind::Base),
        timestamp_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_merge_updates_nested_maps_and_removes_nulls() {
        let mut state = json!({
            "quotes": {
                "SHFE.rb2610": {
                    "last_price": 3100.0,
                    "volume": 100
                }
            }
        });
        merge_diff(
            &mut state,
            json!({
                "quotes": {
                    "SHFE.rb2610": {
                        "last_price": 3105.0,
                        "volume": null
                    }
                }
            }),
        );
        assert_eq!(
            state.pointer("/quotes/SHFE.rb2610/last_price"),
            Some(&json!(3105.0))
        );
        assert_eq!(state.pointer("/quotes/SHFE.rb2610/volume"), None);
    }

    #[test]
    fn catalog_parser_keeps_only_futures_exchanges_and_ranks_main_continuous_first() {
        let nodes = vec![
            json!({
                "class": "FUTURE",
                "exchange_id": "SHFE",
                "instrument_id": "SHFE.rb2610",
                "instrument_name": "螺纹钢2610",
                "product_id": "rb",
                "py_wh": "lwg",
                "english_name": "rebar",
                "price_tick": 1.0,
                "volume_multiple": 10.0
            }),
            json!({
                "class": "CONT",
                "exchange_id": "KQ",
                "instrument_id": "KQ.m@SHFE.rb",
                "instrument_name": "螺纹主连",
                "py_wh": "lwzl",
                "english_name": "rebar main",
                "price_tick": 1.0,
                "volume_multiple": 10.0,
                "underlying": {
                    "edges": [{ "node": { "instrument_id": "SHFE.rb2610" } }]
                }
            }),
            json!({
                "class": "INDEX",
                "exchange_id": "SSE",
                "instrument_id": "SSE.000300",
                "instrument_name": "沪深300"
            }),
        ];
        let assets = parse_catalog_nodes(&nodes);
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].symbol, "KQ.m@SHFE.rb");
        assert_eq!(assets[0].underlying_symbol.as_deref(), Some("SHFE.rb2610"));

        let by_code = search_catalog(&assets, "rb");
        assert_eq!(by_code[0].full_symbol, "KQ.m@SHFE.rb");
        assert_eq!(by_code[1].full_symbol, "SHFE.rb2610");

        let by_chinese = search_catalog(&assets, "螺纹");
        assert_eq!(by_chinese.len(), 2);

        let by_pinyin = search_catalog(&assets, "lwzl");
        assert_eq!(by_pinyin[0].full_symbol, "KQ.m@SHFE.rb");
    }

    #[test]
    fn kline_parser_converts_nanoseconds_to_milliseconds() {
        let raw = json!({
            "datetime": 1_731_632_400_000_000_000_i64,
            "open": 3100.0,
            "high": 3120.0,
            "low": 3095.0,
            "close": 3115.0,
            "volume": 4200
        });
        let kline = parse_kline_entry(&raw, 60_000).unwrap();
        assert_eq!(kline.open_time, 1_731_632_400_000);
        assert_eq!(kline.close_time, 1_731_632_459_999);
        assert_eq!(kline.close, 3115.0);
        assert_eq!(kline.volume, 4200.0);
    }

    #[test]
    #[ignore = "requires DEGENVIEW_TQSDK_USER and DEGENVIEW_TQSDK_PASSWORD"]
    fn live_tqsdk_market_data_smoke() {
        let (user, pass) =
            tqsdk_credentials().expect("set DEGENVIEW_TQSDK_USER and DEGENVIEW_TQSDK_PASSWORD");
        test_connection(&user, &pass).expect("tqsdk auth failed");

        let klines = TQSDK
            .fetch_klines("KQ.m@SHFE.rb", Timeframe::parse("15m").unwrap(), 20)
            .expect("klines failed");
        assert!(!klines.is_empty(), "expected klines for KQ.m@SHFE.rb");

        for sym in ["KQ.m@SHFE.rb", "KQ.m@CFFEX.IF", "SHFE.rb2701"] {
            let daily = crate::service::fetch_klines(DataSource::TqSdk, sym, Timeframe::parse("1d").unwrap(), 500)
                .unwrap_or_else(|e| panic!("daily klines failed for {sym}: {e:?}"));
            assert!(
                daily.len() >= 10,
                "expected >= 10 daily klines for {sym}, got {}",
                daily.len()
            );
        }

        let quotes = quotes(&[QuoteRequest {
            symbol: "KQ.m@SHFE.rb".to_owned(),
            chain: None,
        }])
        .expect("quotes failed");
        assert!(quotes.contains_key("KQ.m@SHFE.rb"), "expected quote");

        let results = TQSDK.search_tickers("rb").expect("search failed");
        assert!(!results.is_empty(), "expected rb search results");
        assert!(
            results.iter().any(|r| r.full_symbol.contains("SHFE.rb")),
            "expected SHFE.rb in search results"
        );
    }
}
