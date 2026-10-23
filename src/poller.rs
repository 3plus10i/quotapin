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
        show_time: cfg.show_time,
        refreshing: false,
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
/// Same ±2 min as the state classifier (`ui::TIME_TOL`) because it is the same
/// question asked about two samples: *time left* grew by more than the clock
/// ran. The bound is `60 < TIME_TOL < INTERVAL_ACTIVE`, locked by
/// `time_tolerance_fits_between_noise_and_cadence`.
const RESET_SLACK: i64 = crate::ui::TIME_TOL;

/// Did the window roll over between two successful samples?
///
/// The test is on **time left**, `L = reset_at − t`, which within one window can
/// only ever shrink:
///
/// ```text
/// ΔL = (R − now) − (R₀ − at₀) = (R − R₀) − (now − at₀) > ε   → 新周期
/// ```
///
/// - A running window: `L` drops by exactly the elapsed time, `ΔL = −Δt`.
/// - An **unused** window: the server re-anchors it to `R = query + H` on every
///   request (DEVDOC §6.2), so `ΔL = 0` — the old absolute test
///   `R − R₀ > 120 s` fired on every poll here, which is the toast storm.
/// - A rollover: the old window is near its end when it flips, so `L` jumps
///   back up to ~`H`, i.e. `ΔL ≈ 5h`. Orders of magnitude above `ε`.
/// - A window that merely *starts* (ready → running): the new anchor is later
///   than the previous query, so `ΔL ≤ 0`. Never reported. ✓
///
/// The second test is the fallback for a server that keeps `reset_at` on a
/// rollover: a drop of 20 points of *used* is a banked/instant reset.
///
/// `prev.at == 0` means the mark predates this check (older state.json); the
/// timestamp signal is skipped for that one poll and the mark is rewritten with
/// a stamp, so the next poll compares normally.
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

/// Banked reset credits went **up**: the account earned one (a referral, a
/// purchase, a grant). Reported once per increase, with the delta and the new
/// total, because "you have 3" says nothing about whether anything happened.
///
/// A drop is deliberately silent — spending a credit is something the user just
/// did, and §6.3 already covers the credits that are about to expire. `None`
/// means we have never looked (fresh install, or a state file from before this
/// check existed), so the first sample only records a baseline; without that,
/// every launch would claim a gain that happened days ago.
fn process_credit_increase(snap: &Snapshot, cfg: &mut Config, toasts: &mut Vec<Toast>) {
    let now = snap.credits_available;
    let Some(prev) = cfg.last_credits else {
        cfg.last_credits = Some(now);
        return;
    };
    if now > prev {
        toasts.push(Toast {
            title: "储备重置机会增加！".into(),
            body: format!("储备重置增加了 {} 次，当前共持有 {} 次。", now - prev, now),
            // good news, painted with the product mark like a reset
            kind: 2,
        });
    }
    cfg.last_credits = Some(now);
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

    // Our own windows (popover, tray menu) become the foreground window when the
    // user opens them. Treating that as "Codex left the foreground" is what made
    // the mini widget vanish the instant you clicked it — so samples where we are
    // the foreground app are skipped and the last real answer stands.
    let own_exe = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
        })
        .unwrap_or_else(|| "quotapin.exe".to_string());

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
            let fg_exe = sys::foreground_exe();
            // we are the foreground app: the user is looking at our own panel,
            // which says nothing about where Codex went. Keep the last answer.
            if fg_exe.as_deref() != Some(own_exe.as_str()) {
                let fg = fg_exe
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
            // Every decision below is about *this* sample, so it is dated by the
            // sample itself (`Snapshot::at`, taken when the request went out) and
            // not by the loop's tick clock. That keeps `Mark::at` and `View::at`
            // the same instant, which is what `rolled_over` and `ui::cycle_state`
            // both assume.
            let now = snap.at;
            let mut toasts: Vec<Toast> = Vec::new();

            {
                let mut c = cfg.lock().unwrap();
                if snap.ok {
                    process_notifications(&snap, &mut c, now, &mut toasts);
                    process_credit_increase(&snap, &mut c, &mut toasts);
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

    /// The tolerance has to sit between the noise floor and the cadence.
    ///
    /// Below the noise floor (~60 s: the timestamps are minute-precision and a
    /// single query may take a minute) an idle window's flat `ΔL` would read as
    /// a jump. Above the fastest normal cadence (180 s) a single poll's worth of
    /// drift would be indistinguishable from a real rollover.
    #[test]
    fn time_tolerance_fits_between_noise_and_cadence() {
        assert!(
            RESET_SLACK > 60,
            "must cover minute-precision stamps plus one slow query"
        );
        assert!(
            RESET_SLACK < INTERVAL_ACTIVE,
            "must stay under the fastest normal poll cadence"
        );
        assert!(INTERVAL_ACTIVE < INTERVAL_IDLE, "active must poll faster");
        assert!(
            INTERVAL_IDLE < 18_000,
            "the slowest poll must be well inside a 5h window"
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

    /// Earning a credit is an event; losing one is not. And the first sample
    /// only sets a baseline, or every launch would announce a gain from days ago.
    #[test]
    fn earning_a_credit_is_announced_once() {
        let mut cfg = Config::default();
        let mut toasts = Vec::new();

        let mut s = snap(0.0, 0.0);
        s.credits_available = 2;
        process_credit_increase(&s, &mut cfg, &mut toasts);
        assert!(toasts.is_empty(), "the first sample is only a baseline");
        assert_eq!(cfg.last_credits, Some(2));

        process_credit_increase(&s, &mut cfg, &mut toasts);
        assert!(toasts.is_empty(), "no change, no toast");

        s.credits_available = 3;
        process_credit_increase(&s, &mut cfg, &mut toasts);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].title, "储备重置机会增加！");
        assert_eq!(toasts[0].body, "储备重置增加了 1 次，当前共持有 3 次。");
        assert_eq!(toasts[0].kind, 2);

        // spending one is silent, and the lower total becomes the new baseline
        s.credits_available = 1;
        process_credit_increase(&s, &mut cfg, &mut toasts);
        assert_eq!(toasts.len(), 1);
        assert_eq!(cfg.last_credits, Some(1));

        // ...so the next gain is measured from 1, not from 3
        s.credits_available = 4;
        process_credit_increase(&s, &mut cfg, &mut toasts);
        assert_eq!(toasts.len(), 2);
        assert_eq!(toasts[1].body, "储备重置增加了 3 次，当前共持有 4 次。");
    }

    #[test]
    fn short_labels_come_from_the_window_length() {
        assert_eq!(short_label(18_000, "5 小时"), "5h");
        assert_eq!(short_label(604_800, "7 天"), "7d");
        assert_eq!(short_label(86_400, "1 天"), "1d");
    }
}
