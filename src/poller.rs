//! Background worker: polling schedule, change detection and notification
//! triggers. All network / disk work happens here so the UI thread only paints.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::api::{self, Snapshot};
use crate::config::{Config, Mark};
use crate::sys;
use crate::ui::{RowView, View};

pub const WM_DATA: u32 = sys::WM_APP + 1;
pub const WM_FOREGROUND: u32 = sys::WM_APP + 3;

/// Poll cadence, per the spec.
const INTERVAL_ACTIVE: i64 = 180; // Codex running on this machine
const INTERVAL_IDLE: i64 = 1800; // Codex not running
const RETRY_AFTER_ERROR: i64 = 60;

pub struct Toast {
    pub title: String,
    pub body: String,
    /// 0 = info, 1 = warning, 2 = branded
    pub kind: u8,
}

#[derive(Default)]
pub struct Shared {
    pub view: View,
    pub toasts: Vec<Toast>,
    pub codex_fg: bool,
    pub force: bool,
}

pub type SharedRef = Arc<Mutex<Shared>>;

pub fn new_shared() -> SharedRef {
    Arc::new(Mutex::new(Shared {
        view: View::default(),
        toasts: Vec::new(),
        codex_fg: false,
        force: false,
    }))
}

fn short_label(limit_secs: i64, label: &str) -> String {
    match limit_secs {
        18_000 => "5h".into(),
        604_800 => "7d".into(),
        s if s > 0 && s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s > 0 && s % 3_600 == 0 => format!("{}h", s / 3_600),
        _ => label.chars().take(3).collect(),
    }
}

fn build_view(snap: &Snapshot, cfg: &Config, last_ok: Option<&Snapshot>, now: i64) -> View {
    let to_row = |r: &crate::api::Row| RowView {
        label: r.label.clone(),
        short: short_label(r.window.limit_secs, &r.label),
        limit_secs: r.window.limit_secs,
        used: r.window.used,
        remaining: r.window.remaining(),
        reset_at: r.window.reset_at,
    };
    let mut v = View {
        rows: snap.rows.iter().map(to_row).collect(),
        credits: snap.credits_available,
        nearest_expiry: snap.nearest_credit_expiry(),
        at: if snap.ok { snap.at } else { last_ok.map(|s| s.at).unwrap_or(0) },
        err: snap.error.clone(),
        err_at: snap.at,
        show_used: cfg.show_used,
        now,
    };
    if v.at == 0 {
        // nothing successful yet: keep whatever the last good snapshot had
        if let Some(l) = last_ok {
            v.rows = l.rows.iter().map(to_row).collect();
            v.credits = l.credits_available;
            v.nearest_expiry = l.nearest_credit_expiry();
        }
    }
    v
}

/// Remaining bucket: ceil(remaining / 20) in 0..=5.
fn bucket(remaining: f64) -> i32 {
    ((remaining / 20.0).ceil() as i32).clamp(0, 5)
}

/// Slack allowed on top of the elapsed time, in seconds.
///
/// It has to satisfy two bounds, checked by `slack_fits_between_polls_and_window`:
///
/// - **slack < fastest poll interval** (60 s). Then one poll's worth of server
///   jitter can never reach the threshold, and a sliding deadline (shift ==
///   elapsed, by definition) can never be read as a jump.
/// - **slowest poll interval < 5 h.** Otherwise a whole window could pass
///   between two samples and a real rollover would look like a slide.
const RESET_SLACK: i64 = 30;

/// A window rolled over if its reset timestamp jumped forward by **more than the
/// time that actually passed**, or — as a fallback for servers that keep the
/// stamp — if usage fell off a cliff.
///
/// The comparison has to be against the clock. An unused 5h window keeps
/// answering `reset_at = now + 5h`: the deadline slides forward by exactly the
/// poll interval on every poll, so an absolute "moved more than 2 minutes" test
/// fires every single time — that is the toast storm where "5 小时额度已重置"
/// arrived every 3 minutes with 已用 0% and the reset time walking forward. A
/// real rollover moves the deadline by the leftover of the old window *plus*
/// the elapsed time, so it stands out by hours, not minutes.
///
/// `prev.at == 0` means the mark predates this check (older state.json); the
/// timestamp signal is skipped for that one poll and the mark is rewritten with
/// a stamp, so the next poll compares normally.
///
/// This is deliberately independent of the bucket: a cycle that only burns 5%
/// of the quota stays in bucket 5 the whole time, so bucket comparison alone
/// would miss the reset entirely.
fn rolled_over(prev: &Mark, used: f64, reset_at: i64, now: i64) -> bool {
    if reset_at > 0 && prev.reset_at > 0 && prev.at > 0 {
        let elapsed = (now - prev.at).max(0);
        let shifted = reset_at - prev.reset_at;
        if shifted - elapsed > RESET_SLACK {
            return true;
        }
    }
    used + 20.0 <= prev.used
}

/// The launch announcement: one toast on the first successful fetch saying the
/// tool is up and what both windows currently hold.
///
/// Tied to the first *successful* poll rather than to process start, so a launch
/// with no network greets once the data actually arrives instead of announcing
/// an empty panel. `App::drain_toasts` drops it when 静默运行 is on.
fn greeting(snap: &Snapshot) -> Toast {
    let mut parts: Vec<String> = Vec::new();
    for r in snap.rows.iter().take(2) {
        parts.push(format!(
            "{} {}",
            short_label(r.window.limit_secs, &r.label),
            crate::fmt::pct(r.window.remaining())
        ));
    }
    if snap.credits_available > 0 {
        parts.push(format!("重置机会 {} 次", snap.credits_available));
    }
    Toast {
        title: "QuotaPin 已启动".into(),
        body: if parts.is_empty() {
            "已连接，正在读取余量".into()
        } else {
            parts.join(" · ")
        },
        kind: 0,
    }
}

fn process_notifications(
    snap: &Snapshot,
    cfg: &mut Config,
    now: i64,
    toasts: &mut Vec<Toast>,
) {
    for row in &snap.rows {
        let used = row.window.used;
        let remaining = row.window.remaining();
        let reset_at = row.window.reset_at;
        let b = bucket(remaining);
        let Some(prev) = cfg.marks.get(&row.label).copied() else {
            // first observation: record silently, never notify on launch
            cfg.marks.insert(
                row.label.clone(),
                Mark { bucket: b, used, reset_at, at: now },
            );
            continue;
        };

        if rolled_over(&prev, used, reset_at, now) {
            toasts.push(Toast {
                title: format!("{}额度已重置", row.label),
                body: format!(
                    "已用 {} · 下次重置 {}",
                    crate::fmt::pct(used),
                    crate::fmt::stamp(reset_at, now)
                ),
                kind: 2,
            });
        } else if b < prev.bucket {
            let node = b * 20;
            let (title, body) = if node == 0 {
                (
                    format!("{}额度已用尽", row.label),
                    format!(
                        "重置 {} · {}",
                        crate::fmt::stamp(reset_at, now),
                        crate::fmt::countdown(reset_at, now)
                    ),
                )
            } else {
                (
                    format!("{}额度剩余 {}%", row.label, node),
                    format!(
                        "已用 {} · 重置 {}（{}）",
                        crate::fmt::pct(used),
                        crate::fmt::stamp(reset_at, now),
                        crate::fmt::countdown(reset_at, now)
                    ),
                )
            };
            toasts.push(Toast {
                title,
                body,
                kind: if node <= 20 { 1 } else { 0 },
            });
        }

        cfg.marks.insert(
            row.label.clone(),
            Mark { bucket: b, used, reset_at, at: now },
        );
    }
}

/// Reset credits expiring within 3 days: one toast per credit, checked once a
/// day at 10:00 local time.
fn process_credit_expiry(
    snap: &Snapshot,
    cfg: &mut Config,
    now: i64,
    toasts: &mut Vec<Toast>,
) {
    let t = sys::local_time();
    let today = sys::local_day_key();
    if cfg.expiry_day == today {
        return;
    }
    if t.wHour != 10 {
        return;
    }

    let three_days = 3 * 86_400;
    let mut due: Vec<(&crate::api::Credit, i64)> = Vec::new();
    for c in snap.available_credits() {
        if c.expires_at <= 0 {
            continue;
        }
        let left = c.expires_at - now;
        if left > 0 && left <= three_days && !cfg.warned_credits.iter().any(|id| id == &c.id) {
            due.push((c, left));
        }
    }

    cfg.expiry_day = today;

    if due.is_empty() {
        return;
    }
    for (c, _) in &due {
        cfg.warned_credits.push(c.id.clone());
    }

    let earliest = due
        .iter()
        .min_by_key(|(_, left)| *left)
        .map(|(c, _)| c)
        .unwrap();
    let body = if due.len() == 1 {
        format!(
            "{} 到期 · {}",
            crate::fmt::stamp(earliest.expires_at, now),
            if earliest.title.is_empty() {
                "重置机会".to_string()
            } else {
                earliest.title.clone()
            }
        )
    } else {
        format!(
            "最早 {} 到期 · 共 {} 次可用",
            crate::fmt::stamp(earliest.expires_at, now),
            due.len()
        )
    };

    toasts.push(Toast {
        title: format!("{} 次重置机会即将到期", due.len()),
        body,
        kind: 1,
    });
}

pub fn spawn(hwnd: HWND, shared: SharedRef, cfg: Arc<Mutex<Config>>) {
    // HWND is a raw pointer and therefore not Send; the numeric value is stable
    // for the lifetime of the window, so it can cross the thread boundary.
    let addr = hwnd as usize;
    std::thread::Builder::new()
        .name("quotapin-poller".into())
        .spawn(move || worker(addr as HWND, shared, cfg))
        .ok();
}

fn worker(hwnd: HWND, shared: SharedRef, cfg: Arc<Mutex<Config>>) {
    let mut next_fetch_at: i64 = 0;
    let mut last_ok: Option<Snapshot> = None;
    let mut failures: i32 = 0;
    let mut last_fg: Option<bool> = None;
    let mut tick: u32 = 0;
    let mut greeted = false;

    loop {
        std::thread::sleep(Duration::from_millis(250));
        tick = tick.wrapping_add(1);
        let now = sys::unix_now();

        // --- foreground watcher (~1 Hz) --------------------------------
        if tick % 4 == 0 {
            let names = {
                let c = cfg.lock().unwrap();
                let mut n = vec![
                    "codex.exe".to_string(),
                    "chatgpt.exe".to_string(),
                    "codex desktop.exe".to_string(),
                ];
                for extra in &c.extra_procs {
                    n.push(extra.clone());
                }
                n
            };
            let fg = sys::foreground_exe()
                .map(|e| names.iter().any(|n| n == &e))
                .unwrap_or(false);
            if last_fg != Some(fg) {
                last_fg = Some(fg);
                if let Ok(mut s) = shared.lock() {
                    s.codex_fg = fg;
                }
                unsafe {
                    PostMessageW(hwnd, WM_FOREGROUND, fg as usize, 0);
                }
            }
        }

        // --- forced refresh from the UI --------------------------------
        let forced = shared
            .lock()
            .map(|mut s| {
                let f = s.force;
                s.force = false;
                f
            })
            .unwrap_or(false);
        if forced {
            next_fetch_at = 0;
        }

        // --- poll ------------------------------------------------------
        if now >= next_fetch_at {
            let active = {
                let c = cfg.lock().unwrap();
                let mut names = vec!["codex.exe".to_string(), "chatgpt.exe".to_string()];
                for extra in &c.extra_procs {
                    names.push(extra.clone());
                }
                sys::any_process_running(&names)
            };

            let snap = api::fetch();
            let mut toasts: Vec<Toast> = Vec::new();

            {
                let mut c = cfg.lock().unwrap();
                if snap.ok {
                    process_notifications(&snap, &mut c, now, &mut toasts);
                    process_credit_expiry(&snap, &mut c, now, &mut toasts);
                    c.save();
                }
            }

            if snap.ok && !greeted {
                greeted = true;
                toasts.push(greeting(&snap));
            }

            if snap.ok {
                last_ok = Some(snap.clone());
                failures = 0;
            } else {
                failures += 1;
            }

            {
                let c = cfg.lock().unwrap();
                let view = build_view(&snap, &c, last_ok.as_ref(), now);
                if let Ok(mut s) = shared.lock() {
                    s.view = view;
                    s.toasts.extend(toasts);
                }
            }

            unsafe {
                PostMessageW(hwnd, WM_DATA, 0, 0);
            }

            next_fetch_at = now
                + if snap.ok {
                    if active {
                        INTERVAL_ACTIVE
                    } else {
                        INTERVAL_IDLE
                    }
                } else if failures <= 3 {
                    RETRY_AFTER_ERROR
                } else if active {
                    INTERVAL_ACTIVE
                } else {
                    INTERVAL_IDLE
                };
        }

        // --- keep "now" fresh so countdowns stay correct ---------------
        if tick % 60 == 0 {
            if let Ok(mut s) = shared.lock() {
                s.view.now = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Row, Snapshot, WindowInfo};

    fn snap(used5: f64, used7: f64) -> Snapshot {
        snap_r(used5, used7, 5_000, 9_000)
    }

    fn snap_r(used5: f64, used7: f64, reset5: i64, reset7: i64) -> Snapshot {
        Snapshot {
            at: 1_000,
            ok: true,
            rows: vec![
                Row {
                    label: "5 小时".into(),
                    window: WindowInfo {
                        used: used5,
                        limit_secs: 18_000,
                        reset_at: reset5,
                    },
                },
                Row {
                    label: "7 天".into(),
                    window: WindowInfo {
                        used: used7,
                        limit_secs: 604_800,
                        reset_at: reset7,
                    },
                },
            ],
            credits_available: 2,
            ..Default::default()
        }
    }

    #[test]
    fn bucket_follows_the_20_percent_nodes() {
        assert_eq!(bucket(100.0), 5);
        assert_eq!(bucket(80.0), 4);
        assert_eq!(bucket(79.9), 4);
        assert_eq!(bucket(60.0), 3);
        assert_eq!(bucket(40.0), 2);
        assert_eq!(bucket(20.0), 1);
        assert_eq!(bucket(0.0), 0);
    }

    #[test]
    fn first_observation_is_silent() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        process_notifications(&snap(20.0, 8.0), &mut cfg, 1_000, &mut toasts);
        assert!(toasts.is_empty(), "launch must not notify");
        assert_eq!(
            cfg.marks.get("5 小时"),
            Some(&Mark {
                bucket: 4,
                used: 20.0,
                reset_at: 5_000,
                at: 1_000
            })
        );
        assert_eq!(cfg.marks.get("7 天").map(|m| m.bucket), Some(5));
    }

    #[test]
    fn crossing_a_node_notifies_once() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        process_notifications(&snap(0.0, 0.0), &mut cfg, 1_000, &mut toasts);
        assert!(toasts.is_empty());

        process_notifications(&snap(20.0, 0.0), &mut cfg, 1_000, &mut toasts);
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].title.contains("80%"));

        // 79.5% remaining is still inside the same bucket -> silent
        process_notifications(&snap(20.5, 0.0), &mut cfg, 1_000, &mut toasts);
        assert_eq!(toasts.len(), 1);

        process_notifications(&snap(40.0, 0.0), &mut cfg, 1_000, &mut toasts);
        assert_eq!(toasts.len(), 2);
        assert!(toasts[1].title.contains("60%"));
    }

    #[test]
    fn running_out_is_a_warning() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        process_notifications(&snap(60.0, 0.0), &mut cfg, 1_000, &mut toasts);
        toasts.clear();
        process_notifications(&snap(100.0, 0.0), &mut cfg, 1_000, &mut toasts);
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].title.contains("已用尽"));
        assert_eq!(toasts[0].kind, 1);
    }

    #[test]
    fn usage_dropping_means_the_window_rolled_over() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        process_notifications(&snap(90.0, 50.0), &mut cfg, 1_000, &mut toasts);
        toasts.clear();
        process_notifications(&snap(5.0, 50.0), &mut cfg, 1_000, &mut toasts);
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].title.contains("已重置"));
        assert_eq!(cfg.marks.get("5 小时").map(|m| m.bucket), Some(5));
    }

    /// The whole point of tracking `reset_at`: a light cycle never leaves
    /// bucket 5, so bucket comparison would stay silent forever.
    #[test]
    fn reset_is_noticed_even_when_usage_never_reached_20_percent() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        // 3% used -> 97% remaining -> bucket 5, and still bucket 5 after reset
        process_notifications(&snap_r(3.0, 2.0, 5_000, 9_000), &mut cfg, 1_000, &mut toasts);
        assert!(toasts.is_empty());

        // the 5h window rolled over; usage only moved 3 -> 1
        process_notifications(&snap_r(1.0, 2.0, 25_000, 9_000), &mut cfg, 3_000, &mut toasts);
        assert_eq!(toasts.len(), 1, "a light cycle must still report the reset");
        assert!(toasts[0].title.contains("已重置"));
        assert_eq!(toasts[0].kind, 2);
        assert_eq!(cfg.marks.get("5 小时").map(|m| m.reset_at), Some(25_000));
    }

    /// Reported 2026-10-20: an unused 5h window answers `reset_at = now + 5h`,
    /// so the deadline walks forward by one poll interval on every poll. That
    /// used to pass the "moved more than 2 minutes" test, and the user got
    /// 「5 小时额度已重置」every 3 minutes with 已用 0% and the reset time
    /// sliding ahead by 3 minutes each time.
    #[test]
    fn a_sliding_deadline_is_not_a_reset() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        let mut now = 1_000;
        let week = 200_000;

        process_notifications(&snap_r(0.0, 20.0, now + 18_000, week), &mut cfg, now, &mut toasts);
        assert!(toasts.is_empty());

        for _ in 0..8 {
            now += 180; // one 3-minute poll
            let reset = now + 18_000;
            process_notifications(&snap_r(0.0, 20.0, reset, week), &mut cfg, now, &mut toasts);
        }
        assert!(toasts.is_empty(), "an idle window must stay silent");
        assert_eq!(cfg.marks["5 小时"].at, now, "the mark keeps a fresh stamp");

        // ...and a genuine rollover right after that still gets through: the
        // old window ends now, the new one adds a full 5h on top of it.
        now += 180;
        process_notifications(
            &snap_r(0.0, 20.0, now + 18_000 + 5_400, week),
            &mut cfg,
            now,
            &mut toasts,
        );
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].title.contains("已重置"));
    }

    /// The two bounds `RESET_SLACK` depends on. Both are properties of the
    /// schedule, so they belong in a test: if someone shortens the idle cadence
    /// below the slack, or raises the slack above the fastest retry, the
    /// rollover test silently starts lying again.
    #[test]
    fn slack_fits_between_polls_and_window() {
        assert!(
            RESET_SLACK < RETRY_AFTER_ERROR,
            "slack must stay under the fastest poll interval, or one poll of \
             server jitter can reach the threshold"
        );
        assert!(
            INTERVAL_IDLE < 18_000,
            "the slowest poll must be well inside a 5h window, or a real \
             rollover looks like a slide"
        );
        assert!(
            INTERVAL_IDLE > RETRY_AFTER_ERROR && INTERVAL_ACTIVE > RETRY_AFTER_ERROR,
            "a normal cadence must be slower than the error retry"
        );
    }

    /// The launch toast is the only notification that is not an event: it has
    /// to carry both windows at once, and it must not look like a warning.
    #[test]
    fn the_launch_toast_names_both_windows() {
        let t = greeting(&snap(20.0, 8.0));
        assert_eq!(t.title, "QuotaPin 已启动");
        assert_eq!(t.body, "5h 80% · 7d 92% · 重置机会 2 次");
        assert_eq!(t.kind, 0);

        // no credits -> do not claim "0 次"
        let mut none = snap(0.0, 0.0);
        none.credits_available = 0;
        assert_eq!(greeting(&none).body, "5h 100% · 7d 100%");

        // a successful fetch without rows still says something
        let empty = Snapshot { at: 1, ok: true, ..Default::default() };
        assert!(!greeting(&empty).body.is_empty());
    }

    #[test]
    fn a_quiet_poll_never_notifies() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();
        process_notifications(&snap(31.0, 12.0), &mut cfg, 1_000, &mut toasts);
        for _ in 0..5 {
            process_notifications(&snap(31.0, 12.0), &mut cfg, 2_000, &mut toasts);
        }
        assert!(toasts.is_empty());
    }

    #[test]
    fn short_labels_come_from_the_window_length() {
        assert_eq!(short_label(18_000, "5 小时"), "5h");
        assert_eq!(short_label(604_800, "7 天"), "7d");
        assert_eq!(short_label(86_400, "1 天"), "1d");
    }
}
