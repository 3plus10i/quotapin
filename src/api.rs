//! Data layer: reads the Codex desktop credentials from `auth.json` and calls
//! the same internal endpoints the reference PowerShell script uses.
//!
//!   GET https://chatgpt.com/backend-api/wham/usage
//!   GET https://chatgpt.com/backend-api/wham/rate-limit-reset-credits
//!
//! Only the fields the script printed are modelled here. Nothing is invented.

use std::ffi::c_void;

use windows_sys::Win32::Networking::WinHttp::*;

use crate::sys;

pub const HOST: &str = "chatgpt.com";

#[derive(Clone, Debug, Default)]
pub struct WindowInfo {
    pub used: f64,
    pub limit_secs: i64,
    pub reset_at: i64,
}

impl WindowInfo {
    pub fn remaining(&self) -> f64 {
        (100.0 - self.used).clamp(0.0, 100.0)
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    /// "5 小时" / "7 天" / additional limit name
    pub label: String,
    pub window: WindowInfo,
}

#[derive(Clone, Debug)]
pub struct Credit {
    pub id: String,
    pub title: String,
    pub status: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub at: i64,
    pub rows: Vec<Row>,
    pub credits: Vec<Credit>,
    pub credits_available: i64,
    pub plan: String,
    pub error: Option<String>,
    /// true when the last fetch succeeded (rows/credits are valid)
    pub ok: bool,
}

impl Snapshot {
    /// Earliest expiry among the currently available reset credits.
    pub fn nearest_credit_expiry(&self) -> Option<i64> {
        self.credits
            .iter()
            .filter(|c| c.status == "available")
            .map(|c| c.expires_at)
            .min()
    }

    pub fn available_credits(&self) -> Vec<&Credit> {
        self.credits
            .iter()
            .filter(|c| c.status == "available")
            .collect()
    }
}

// ---------------------------------------------------------------- auth

struct Auth {
    token: String,
    account: Option<String>,
}

fn load_auth() -> Result<Auth, String> {
    let path = sys::codex_auth_path();
    let raw = std::fs::read_to_string(&path)
        .map_err(|_| format!("找不到 Codex 凭证：{}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("auth.json 解析失败：{e}"))?;
    let token = v
        .get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "auth.json 中没有 tokens.access_token，请先用 ChatGPT 账号登录 Codex".to_string())?;
    let account = v
        .get("tokens")
        .and_then(|t| t.get("account_id"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    Ok(Auth {
        token: token.to_string(),
        account,
    })
}

// ---------------------------------------------------------------- http

struct HttpResponse {
    status: u32,
    body: String,
}

unsafe fn close(h: *mut c_void) {
    if !h.is_null() {
        WinHttpCloseHandle(h);
    }
}

fn http_get(path: &str, headers: &[(String, String)]) -> Result<HttpResponse, String> {
    let agent = sys::wide("QuotaPin/0.1 (Windows)");
    let host = sys::wide(HOST);
    let verb = sys::wide("GET");
    let object = sys::wide(path);

    let mut header_block = String::new();
    for (k, v) in headers {
        header_block.push_str(k);
        header_block.push_str(": ");
        header_block.push_str(v);
        header_block.push_str("\r\n");
    }

    unsafe {
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen 失败".into());
        }
        // resolve=5s, connect=5s, send=8s, receive=10s
        WinHttpSetTimeouts(session, 5000, 5000, 8000, 10000);

        let connect = WinHttpConnect(session, host.as_ptr(), 443, 0);
        if connect.is_null() {
            close(session);
            return Err("连接 chatgpt.com 失败".into());
        }

        let req = WinHttpOpenRequest(
            connect,
            verb.as_ptr(),
            object.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if req.is_null() {
            close(connect);
            close(session);
            return Err("创建请求失败".into());
        }

        let wide_block = sys::wide(&header_block);
        // dwHeadersLength is counted in characters, not bytes
        let hdr_len = (wide_block.len() - 1) as u32;
        let send_ok = WinHttpSendRequest(
            req,
            wide_block.as_ptr(),
            hdr_len,
            std::ptr::null(),
            0,
            0,
            0,
        );
        if send_ok == 0 {
            let e = sys::last_error();
            close(req);
            close(connect);
            close(session);
            return Err(format!("发送请求失败（{e}）"));
        }

        if WinHttpReceiveResponse(req, std::ptr::null_mut()) == 0 {
            let e = sys::last_error();
            close(req);
            close(connect);
            close(session);
            return Err(format!("读取响应失败（{e}）"));
        }

        let mut status: u32 = 0;
        let mut len: u32 = 4;
        WinHttpQueryHeaders(
            req,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
        );

        let mut body: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 8192];
        loop {
            let mut read: u32 = 0;
            let ok = WinHttpReadData(
                req,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut read,
            );
            if ok == 0 || read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > 2 * 1024 * 1024 {
                break;
            }
        }

        close(req);
        close(connect);
        close(session);

        Ok(HttpResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

// ---------------------------------------------------------------- parsing

fn num(v: Option<&serde_json::Value>) -> Option<f64> {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn window_from(v: Option<&serde_json::Value>) -> Option<WindowInfo> {
    let v = v?;
    let used = num(v.get("used_percent"))?;
    Some(WindowInfo {
        used,
        limit_secs: num(v.get("limit_window_seconds")).unwrap_or(0.0) as i64,
        reset_at: num(v.get("reset_at")).unwrap_or(0.0) as i64,
    })
}

pub(crate) fn window_label(secs: i64) -> String {
    if secs == 18_000 {
        return "5 小时".into();
    }
    if secs == 604_800 {
        return "7 天".into();
    }
    if secs > 0 && secs % 86_400 == 0 {
        return format!("{} 天", secs / 86_400);
    }
    if secs > 0 && secs % 3_600 == 0 {
        return format!("{} 小时", secs / 3_600);
    }
    if secs > 0 {
        return format!("{} 秒", secs);
    }
    "未知".into()
}

fn parse_credits(raw: &str) -> (Vec<Credit>, i64) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (Vec::new(), 0);
    };
    let mut out = Vec::new();
    if let Some(list) = v.get("credits").and_then(|c| c.as_array()) {
        for item in list {
            let id = item
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let title = item
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let status = item
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let expires_at = item
                .get("expires_at")
                .and_then(|x| x.as_str())
                .map(parse_iso8601)
                .unwrap_or(0);
            out.push(Credit {
                id,
                title,
                status,
                expires_at,
            });
        }
    }
    let count = num(v.get("available_count"))
        .map(|f| f as i64)
        .unwrap_or_else(|| out.iter().filter(|c| c.status == "available").count() as i64);
    (out, count)
}

/// Minimal ISO-8601 ("2026-10-22T18:13:55.211425Z") -> unix seconds.
fn parse_iso8601(s: &str) -> i64 {
    let b = s.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let get = |r: std::ops::Range<usize>| -> i64 {
        std::str::from_utf8(&b[r])
            .ok()
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (y, mo, d) = (get(0..4), get(5..7), get(8..10));
    let (h, mi, sec) = (get(11..13), get(14..16), get(17..19));

    // days since epoch (Howard Hinnant's algorithm)
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    days * 86_400 + h * 3_600 + mi * 60 + sec
}

// ---------------------------------------------------------------- fetch

pub fn fetch() -> Snapshot {
    let mut snap = Snapshot {
        at: sys::unix_now(),
        ..Default::default()
    };

    let auth = match load_auth() {
        Ok(a) => a,
        Err(e) => {
            snap.error = Some(e);
            return snap;
        }
    };

    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {}", auth.token)),
        ("Accept".to_string(), "application/json".to_string()),
        ("OpenAI-Beta".to_string(), "codex-1".to_string()),
        ("originator".to_string(), "Codex Desktop".to_string()),
        ("OAI-Product-Sku".to_string(), "CODEX".to_string()),
    ];
    if let Some(acc) = &auth.account {
        headers.push(("ChatGPT-Account-Id".to_string(), acc.clone()));
    }

    let usage = match http_get("/backend-api/wham/usage", &headers) {
        Ok(r) => r,
        Err(e) => {
            snap.error = Some(e);
            return snap;
        }
    };

    if usage.status != 200 {
        snap.error = Some(match usage.status {
            401 => "HTTP 401：登录已失效，请重新登录 Codex".to_string(),
            403 => "HTTP 403：当前账号无权访问该接口".to_string(),
            404 => "HTTP 404：接口路径可能已变化".to_string(),
            s => format!("请求失败：HTTP {s}"),
        });
        return snap;
    }

    let v: serde_json::Value = match serde_json::from_str(&usage.body) {
        Ok(v) => v,
        Err(e) => {
            snap.error = Some(format!("响应解析失败：{e}"));
            return snap;
        }
    };

    let rl = v.get("rate_limit");

    if let Some(w) = window_from(rl.and_then(|r| r.get("primary_window"))) {
        snap.rows.push(Row {
            label: window_label(w.limit_secs),
            window: w,
        });
    }
    if let Some(w) = window_from(rl.and_then(|r| r.get("secondary_window"))) {
        snap.rows.push(Row {
            label: window_label(w.limit_secs),
            window: w,
        });
    }

    if let Some(list) = v.get("additional_rate_limits").and_then(|a| a.as_array()) {
        for item in list {
            let name = item
                .get("limit_name")
                .and_then(|x| x.as_str())
                .or_else(|| item.get("metered_feature").and_then(|x| x.as_str()))
                .unwrap_or("额外额度")
                .to_string();
            if let Some(w) = window_from(item.get("rate_limit").and_then(|r| r.get("primary_window"))) {
                snap.rows.push(Row {
                    label: name,
                    window: w,
                });
            }
        }
    }

    if let Some(p) = v.get("plan_type").and_then(|x| x.as_str()) {
        snap.plan = p.to_string();
    }

    // credits: prefer the detailed endpoint, fall back to the usage summary
    let summary_count = num(v.get("rate_limit_reset_credits").and_then(|c| c.get("available_count")))
        .map(|f| f as i64);

    match http_get("/backend-api/wham/rate-limit-reset-credits", &headers) {
        Ok(r) if r.status == 200 => {
            let (credits, count) = parse_credits(&r.body);
            snap.credits = credits;
            snap.credits_available = if summary_count.is_some() {
                summary_count.unwrap_or(count)
            } else {
                count
            };
        }
        _ => {
            snap.credits_available = summary_count.unwrap_or(0);
        }
    }

    snap.ok = !snap.rows.is_empty();
    if !snap.ok {
        snap.error = Some("响应中没有 5 小时 / 7 天窗口数据".into());
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamps_parse_to_unix_seconds() {
        assert_eq!(parse_iso8601("2026-10-22T18:13:55.211425Z"), 1_792_692_835);
        assert_eq!(parse_iso8601("2026-01-01T00:00:00Z"), 1_767_225_600);
        assert_eq!(parse_iso8601("2024-02-29T12:00:00Z"), 1_709_208_000);
        assert_eq!(parse_iso8601("nope"), 0);
    }

    #[test]
    fn window_lengths_become_human_labels() {
        assert_eq!(window_label(18_000), "5 小时");
        assert_eq!(window_label(604_800), "7 天");
        assert_eq!(window_label(86_400), "1 天");
        assert_eq!(window_label(3_600), "1 小时");
        assert_eq!(window_label(0), "未知");
    }

    #[test]
    fn remaining_is_clamped() {
        let w = WindowInfo {
            used: 120.0,
            limit_secs: 18_000,
            reset_at: 0,
        };
        assert_eq!(w.remaining(), 0.0);
        let w = WindowInfo {
            used: -5.0,
            limit_secs: 18_000,
            reset_at: 0,
        };
        assert_eq!(w.remaining(), 100.0);
    }
}
