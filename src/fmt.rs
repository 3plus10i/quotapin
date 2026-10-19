//! Human-friendly formatting of percentages and timestamps (all local time).

use crate::sys;

pub fn pct(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if (r - r.round()).abs() < 0.05 {
        format!("{:.0}%", r)
    } else {
        format!("{:.1}%", r)
    }
}

/// "2 小时 46 分" / "3 天 4 小时" / "12 分" / "不到 1 分"
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    let days = s / 86_400;
    let hours = (s % 86_400) / 3_600;
    let mins = (s % 3_600) / 60;
    if days > 0 {
        if hours > 0 {
            format!("{days} 天 {hours} 小时")
        } else {
            format!("{days} 天")
        }
    } else if hours > 0 {
        if mins > 0 && days == 0 {
            format!("{hours} 小时 {mins} 分")
        } else {
            format!("{hours} 小时")
        }
    } else if mins > 0 {
        format!("{mins} 分")
    } else {
        "不到 1 分".into()
    }
}

pub fn countdown(reset_at: i64, now: i64) -> String {
    if reset_at <= 0 {
        return "重置时间未知".into();
    }
    let left = reset_at - now;
    if left <= 0 {
        "即将重置".into()
    } else {
        format!("{}后", duration(left))
    }
}

fn same_local_day(a: i64, now: i64) -> bool {
    let x = sys::local_from_unix(a);
    let y = sys::local_from_unix(now);
    x.wYear == y.wYear && x.wMonth == y.wMonth && x.wDay == y.wDay
}

/// "14:37" for today, "10-22 18:13" otherwise.
pub fn stamp(ts: i64, now: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    let t = sys::local_from_unix(ts);
    if same_local_day(ts, now) {
        format!("{:02}:{:02}", t.wHour, t.wMinute)
    } else {
        format!("{:02}-{:02} {:02}:{:02}", t.wMonth, t.wDay, t.wHour, t.wMinute)
    }
}

/// "刚刚" / "3 分钟前" / "2 小时前"
pub fn ago(ts: i64, now: i64) -> String {
    let d = now - ts;
    if d < 45 {
        "刚刚".into()
    } else if d < 3_600 {
        format!("{} 分钟前", (d / 60).max(1))
    } else if d < 86_400 {
        format!("{} 小时前", d / 3_600)
    } else {
        format!("{} 天前", d / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_drop_the_noise() {
        assert_eq!(pct(80.0), "80%");
        assert_eq!(pct(79.96), "80%");
        assert_eq!(pct(79.4), "79.4%");
        assert_eq!(pct(0.0), "0%");
    }

    #[test]
    fn durations_pick_the_right_unit() {
        assert_eq!(duration(0), "不到 1 分");
        assert_eq!(duration(59), "不到 1 分");
        assert_eq!(duration(60), "1 分");
        assert_eq!(duration(9_960), "2 小时 46 分");
        assert_eq!(duration(3_600), "1 小时");
        assert_eq!(duration(86_400), "1 天");
        assert_eq!(duration(86_400 + 4 * 3_600), "1 天 4 小时");
    }

    #[test]
    fn countdown_handles_past_deadlines() {
        assert_eq!(countdown(0, 0), "重置时间未知");
        assert_eq!(countdown(100, 200), "即将重置");
        assert_eq!(countdown(3_700, 100), "1 小时后");
    }
}
