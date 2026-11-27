//! ログ（`<home>\logs\scopula-YYYY-MM-DD.log` へ日ごとに追記）と表示用の整形。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Win32 の SYSTEMTIME。
#[repr(C)]
#[derive(Default)]
struct SysTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

unsafe extern "system" {
    fn GetLocalTime(t: *mut SysTime);
}

/// ローカル時刻 `(YYYY-MM-DD, HH:MM:SS)`。
fn local_now() -> (String, String) {
    let mut t = SysTime::default();
    // SAFETY: SYSTEMTIME と同じ配置の構造体を渡している。
    unsafe { GetLocalTime(&mut t) };
    (
        format!("{:04}-{:02}-{:02}", t.year, t.month, t.day),
        format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second),
    )
}

/// 実行ごとの識別子（開始時刻とプロセス ID）。
pub fn new_run_id() -> String {
    let (d, t) = local_now();
    format!(
        "{}-{}-{}",
        d.replace('-', ""),
        t.replace(':', ""),
        std::process::id()
    )
}

/// バイト数を GiB（小数1桁）などで表す。
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub struct Logger {
    dir: PathBuf,
    run_id: String,
}

impl Logger {
    pub fn new(dir: PathBuf, run_id: String) -> Logger {
        Logger { dir, run_id }
    }

    /// 1行を追記する。書けなくても掃除は止めず、標準エラーに出す。
    pub fn line(&self, msg: &str) {
        let (date, time) = local_now();
        let path = self.dir.join(format!("scopula-{date}.log"));
        let result = fs::create_dir_all(&self.dir).and_then(|_| {
            let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
            writeln!(f, "{date} {time} [{}] {msg}", self.run_id)
        });
        if let Err(e) = result {
            eprintln!("ログを書けません ({}): {e}", path.display());
        }
    }

    /// 保持日数より古いログファイル（削除は呼び出し側が Prunlet で行う）。
    pub fn expired_logs(&self, keep_days: u64) -> Vec<PathBuf> {
        let limit = Duration::from_secs(keep_days * 86_400);
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > limit);
                name.starts_with("scopula-") && name.ends_with(".log") && old
            })
            .map(|e| e.path())
            .collect()
    }
}
