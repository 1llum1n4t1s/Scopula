//! 設定。既定値をコードに持ち、`<home>\config.toml`（既定の home は `%LOCALAPPDATA%\Scopula`）で上書きできる。

use serde::Deserialize;
use std::path::{Path, PathBuf};

const GIB: u64 = 1024 * 1024 * 1024;
const DAY: u64 = 86_400;

/// 掃除の判定に使う閾値。判定ロジックはこの値だけに依存する（テストで直接組み立てる）。
#[derive(Debug, Clone)]
pub struct Limits {
    /// 直近この秒数以内に更新があれば、その target は触らない。
    pub recent_secs: u64,
    /// R1: 旧配置（deps・.fingerprint・build\<pkg>-<hash>）全体の最新ファイルがこれより古いとき。
    pub legacy_secs: u64,
    /// R2: incremental\* の経過。
    pub incremental_secs: u64,
    /// R3: ユニットを消せる最低の経過（指紋と全ファイルの両方）。
    pub unit_secs: u64,
    /// R3: P の最終ビルドからこの幅の中のユニットを「使用中」の起点にする。
    pub seed_secs: u64,
    /// R4: target\tmp\* の経過。
    pub tmp_secs: u64,
    /// R6: P 全体の最新ファイルがこれより古ければ、中間成果物を丸ごと消す。
    pub stale_profile_secs: u64,
    /// R5: target の上限。
    pub cap_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Config::default().limits()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 直近この時間以内に更新されたファイルがある target は掃除しない。
    pub recent_hours: u64,
    /// R1: 旧配置全体の経過日数。
    pub legacy_days: u64,
    /// R2: incremental の経過日数。
    pub incremental_days: u64,
    /// R3: ユニットを消せる最低の経過日数。
    pub unit_days: u64,
    /// R3: 最終ビルドからこの日数の幅を「使用中」の起点にする。
    pub seed_days: u64,
    /// R4: target\tmp\* の経過日数。
    pub tmp_days: u64,
    /// R6: P 全体が更新されていない日数。
    pub stale_profile_days: u64,
    /// R5: target の上限（GiB）。
    pub cap_gib: u64,
    /// 掃除後もこれを超える target があれば終了コード 1。
    pub warn_gib: u64,
    /// 探索から外すフォルダー（接頭辞一致）。指定すると既定の一覧を置き換える。
    pub exclude: Vec<String>,
    /// es.exe のパス。空なら PATH 上の `es.exe`、なければ WinGet の既定位置
    /// （`%LOCALAPPDATA%\Microsoft\WinGet\Packages\voidtools.Everything.Cli_*`）。
    pub es_path: String,
    /// es.exe の応答待ち（ミリ秒）。
    pub es_timeout_ms: u64,
    pub prunlet_exe: String,
    pub prunlet_config: String,
    /// Prunlet の1回の削除に渡す件数（上限 1024）。
    pub prunlet_batch: usize,
    /// Prunlet 1回の呼び出しの打ち切り（秒）。超えたら `ambiguous` として記録し、再送しない。
    pub prunlet_timeout_secs: u64,
    /// この日数より古いログを Prunlet で消す。
    pub log_keep_days: u64,
}

/// 環境変数 `var` のフォルダーに `rest` を足したパス。環境変数が無ければ `rest` だけ（実行時に失敗する）。
fn env_path(var: &str, rest: &str) -> String {
    match std::env::var_os(var) {
        Some(base) => PathBuf::from(base).join(rest).display().to_string(),
        None => rest.to_string(),
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            recent_hours: 2,
            legacy_days: 7,
            incremental_days: 7,
            unit_days: 7,
            seed_days: 7,
            tmp_days: 3,
            stale_profile_days: 30,
            cap_gib: 80,
            warn_gib: 100,
            exclude: Vec::new(),
            es_path: String::new(),
            es_timeout_ms: 60_000,
            prunlet_exe: env_path("LOCALAPPDATA", r"PrunletApp\current\prunlet.exe"),
            prunlet_config: env_path("USERPROFILE", r".local\Prunlet\config.json"),
            prunlet_batch: 1000,
            prunlet_timeout_secs: 600,
            log_keep_days: 30,
        }
    }
}

impl Config {
    pub fn limits(&self) -> Limits {
        Limits {
            recent_secs: self.recent_hours * 3600,
            legacy_secs: self.legacy_days * DAY,
            incremental_secs: self.incremental_days * DAY,
            unit_secs: self.unit_days * DAY,
            seed_secs: self.seed_days * DAY,
            tmp_secs: self.tmp_days * DAY,
            stale_profile_secs: self.stale_profile_days * DAY,
            cap_bytes: self.cap_gib * GIB,
        }
    }

    pub fn warn_bytes(&self) -> u64 {
        self.warn_gib * GIB
    }

    /// 設定の食い違いの警告（実行は止めない）。
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.warn_gib < self.cap_gib {
            out.push(format!(
                "warn_gib ({}) が cap_gib ({}) より小さいため、掃除後でも警告になる target が出ます",
                self.warn_gib, self.cap_gib
            ));
        }
        out
    }

    /// 設定ファイルを読む。無ければ既定値。壊れていればエラー。
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// 設定・ログの置き場。`--home` の指定があればそれ、なければ `%LOCALAPPDATA%\Scopula`。
pub fn home_dir(over: Option<&Path>) -> PathBuf {
    if let Some(p) = over {
        return p.to_path_buf();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Scopula")
}
