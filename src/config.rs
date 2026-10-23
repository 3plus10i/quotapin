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
    /// when this mark was taken (unix seconds). Needed to tell a deadline that
    /// jumps apart from one the server simply slides forward as time passes —
    /// an idle 5h window keeps answering `reset_at = now + 5h`, so the two only
    /// differ by *how far* it moved relative to the clock.
    pub at: i64,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// remembered position of the mini widget (screen coords, top-left)
    pub mini_pos: Option<(i32, i32)>,
    /// pin the mini widget even when Codex is not in the foreground
    pub pinned: bool,
    /// 静默运行: drop notifications instead of showing them
    pub muted: bool,
    /// show "used" instead of "remaining" in the big number
    pub show_used: bool,
    /// last seen state per window key ("5 小时" -> Mark)
    pub marks: BTreeMap<String, Mark>,
    /// reset-credit ids we already warned about
    pub warned_credits: Vec<String>,
    /// how many banked reset credits the last successful poll saw. `None` means
    /// "never looked", which is why a fresh install does not announce a gain it
    /// did not witness (see `poller::process_credit_increase`).
    pub last_credits: Option<i64>,
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
            muted: false,
            show_used: false,
            marks: BTreeMap::new(),
            warned_credits: Vec::new(),
            last_credits: None,
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
        let Ok(raw) = std::fs::read_to_string(path()) else {
            return Self::default();
        };
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => Self::from_value(&v),
            Err(_) => Self::default(),
        }
    }

    /// Parse the on-disk shape. Every field is optional, so a file written by an
    /// older build (or edited by hand) still loads and just takes defaults.
    fn from_value(v: &serde_json::Value) -> Self {
        let mut c = Self::default();
        if let Some(arr) = v.get("mini_pos").and_then(|x| x.as_array()) {
            if arr.len() == 2 {
                if let (Some(x), Some(y)) = (arr[0].as_i64(), arr[1].as_i64()) {
                    c.mini_pos = Some((x as i32, y as i32));
                }
            }
        }
        c.pinned = v.get("pinned").and_then(|x| x.as_bool()).unwrap_or(false);
        c.muted = v.get("muted").and_then(|x| x.as_bool()).unwrap_or(false);
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
                            // absent in files from before the sliding-window fix;
                            // 0 means "gap unknown", handled in poller::rolled_over
                            at: val.get("at").and_then(|x| x.as_i64()).unwrap_or(0),
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
        c.last_credits = v.get("last_credits").and_then(|x| x.as_i64());
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
        let body = serde_json::to_string_pretty(&self.to_value()).unwrap_or_else(|_| "{}".into());
        let tmp = dir.join("state.json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("state.json"));
        }
    }

    fn to_value(&self) -> serde_json::Value {
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
                        "at": m.at,
                    }),
                )
            })
            .collect();
        serde_json::json!({
            "mini_pos": self.mini_pos.map(|(x, y)| serde_json::json!([x, y])),
            "pinned": self.pinned,
            "muted": self.muted,
            "show_used": self.show_used,
            "marks": marks,
            "warned_credits": self.warned_credits,
            "last_credits": self.last_credits,
            "expiry_day": self.expiry_day,
            "extra_procs": self.extra_procs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 静默运行 has to survive a restart, and a file written before it existed
    /// has to load as "not muted" instead of failing the whole parse.
    #[test]
    fn muting_round_trips_and_old_files_default_to_loud() {
        let mut c = Config::default();
        c.muted = true;
        assert!(Config::from_value(&c.to_value()).muted);

        let legacy = serde_json::json!({ "pinned": true, "show_used": false });
        let parsed = Config::from_value(&legacy);
        assert!(!parsed.muted);
        assert!(parsed.pinned);
    }

    #[test]
    fn marks_and_credits_still_make_the_round_trip() {
        let mut c = Config::default();
        c.marks.insert(
            "5 小时".into(),
            Mark {
                bucket: 4,
                used: 21.0,
                reset_at: 1_792_414_440,
                at: 1_792_414_000,
            },
        );
        c.warned_credits.push("abc".into());
        c.last_credits = Some(3);
        c.mini_pos = Some((10, 20));
        let back = Config::from_value(&c.to_value());
        assert_eq!(back.marks["5 小时"].bucket, 4);
        assert_eq!(back.marks["5 小时"].used, 21.0);
        assert_eq!(back.warned_credits, vec!["abc".to_string()]);
        assert_eq!(back.mini_pos, Some((10, 20)));
        // the credit baseline survives a restart, otherwise the next launch
        // would re-announce the last gain
        assert_eq!(back.last_credits, Some(3));
    }
}
