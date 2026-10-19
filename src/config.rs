//! Persisted UI state + notification bookkeeping.
//!
//! Kept in `%APPDATA%\QuotaPin\state.json` so it survives restarts and so the
//! notification throttles are not reset every launch.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::sys;

/// What the previous poll saw for one quota window. Two independent signals are
/// needed: the 20% bucket drives the "quota dropped to N%" toasts, while
/// `reset_at` / `used` are what actually prove a window rolled over — a cycle
/// that never burns 20% never changes bucket, yet still resets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mark {
    /// ceil(remaining / 20) in 0..=5
    pub bucket: i32,
    pub used: f64,
    pub reset_at: i64,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// remembered position of the mini widget (screen coords, top-left)
    pub mini_pos: Option<(i32, i32)>,
    /// pin the mini widget even when Codex is not in the foreground
    pub pinned: bool,
    /// show "used" instead of "remaining" in the big number
    pub show_used: bool,
    /// last seen state per window key ("5 小时" -> Mark)
    pub marks: BTreeMap<String, Mark>,
    /// reset-credit ids we already warned about
    pub warned_credits: Vec<String>,
    /// last local day the expiry check ran ("2026-10-19")
    pub expiry_day: String,
    /// extra foreground executable names that should raise the mini widget
    pub extra_procs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mini_pos: None,
            pinned: false,
            show_used: false,
            marks: BTreeMap::new(),
            warned_credits: Vec::new(),
            expiry_day: String::new(),
            extra_procs: Vec::new(),
        }
    }
}

pub fn path() -> PathBuf {
    sys::app_data_dir().join("state.json")
}

impl Config {
    pub fn load() -> Self {
        let p = path();
        let Ok(raw) = std::fs::read_to_string(&p) else {
            return Self::default();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Self::default();
        };
        let mut c = Self::default();
        if let Some(arr) = v.get("mini_pos").and_then(|x| x.as_array()) {
            if arr.len() == 2 {
                if let (Some(x), Some(y)) = (arr[0].as_i64(), arr[1].as_i64()) {
                    c.mini_pos = Some((x as i32, y as i32));
                }
            }
        }
        c.pinned = v.get("pinned").and_then(|x| x.as_bool()).unwrap_or(false);
        c.show_used = v.get("show_used").and_then(|x| x.as_bool()).unwrap_or(false);
        if let Some(obj) = v.get("marks").and_then(|x| x.as_object()) {
            for (k, val) in obj {
                let bucket = val.get("bucket").and_then(|x| x.as_i64());
                if let Some(b) = bucket {
                    c.marks.insert(
                        k.clone(),
                        Mark {
                            bucket: b as i32,
                            used: val.get("used").and_then(|x| x.as_f64()).unwrap_or(0.0),
                            reset_at: val.get("reset_at").and_then(|x| x.as_i64()).unwrap_or(0),
                        },
                    );
                }
            }
        }
        if let Some(arr) = v.get("warned_credits").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    c.warned_credits.push(s.to_string());
                }
            }
        }
        c.expiry_day = v
            .get("expiry_day")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(arr) = v.get("extra_procs").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    c.extra_procs.push(s.to_ascii_lowercase());
                }
            }
        }
        c
    }

    pub fn save(&self) {
        let dir = sys::app_data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let marks: serde_json::Map<String, serde_json::Value> = self
            .marks
            .iter()
            .map(|(k, m)| {
                (
                    k.clone(),
                    serde_json::json!({
                        "bucket": m.bucket,
                        "used": m.used,
                        "reset_at": m.reset_at,
                    }),
                )
            })
            .collect();
        let v = serde_json::json!({
            "mini_pos": self.mini_pos.map(|(x, y)| serde_json::json!([x, y])),
            "pinned": self.pinned,
            "show_used": self.show_used,
            "marks": marks,
            "warned_credits": self.warned_credits,
            "expiry_day": self.expiry_day,
            "extra_procs": self.extra_procs,
        });
        let body = serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into());
        let tmp = dir.join("state.json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("state.json"));
        }
    }
}
