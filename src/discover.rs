//! target フォルダーの探索。既定は Everything（es.exe）、`--root` 指定時はフォルダーの走査。

use crate::config::Config;
use serde::Deserialize;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// 子プロセスの窓を出さない（タスクスケジューラーからの起動でも窓が出ないように）。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 走査で下りないフォルダー名。
const WALK_SKIP: [&str; 3] = [".git", "node_modules", "$recycle.bin"];

/// 正規化して比べるための小文字・バックスラッシュ区切りの文字列。
fn norm(p: &str) -> String {
    p.replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|r| r.starts_with('\\'))
}

/// 探索対象から外すか。`.git`・`node_modules` の下、ごみ箱、設定の除外フォルダーの下。
pub fn is_excluded(path: &Path, exclude: &[String]) -> bool {
    let p = norm(&path.to_string_lossy());
    p.contains("\\.git\\")
        || p.contains("\\node_modules\\")
        || p.contains("\\$recycle.bin\\")
        || exclude.iter().any(|e| under(&p, &norm(e)))
}

/// cargo の target か。`CACHEDIR.TAG` の先頭に cargo の署名と「created by cargo」があること。
/// （uv など他のツールの同名ファイルや、`debug` フォルダーの有無では判定しない。）
pub fn is_target_dir(dir: &Path) -> bool {
    crate::scan::file_is_cargo_tag(&dir.join("CACHEDIR.TAG"))
}

/// 候補（CACHEDIR.TAG のあるフォルダー）から、除外・重複・別の target の内側を除いて target を決める。
/// `resolve` は実体のパスへ直す（ジャンクション経由の別名を同一視するため。存在しなければ `None`）。
pub fn select_targets(
    candidates: Vec<PathBuf>,
    exclude: &[String],
    resolve: impl Fn(&Path) -> Option<PathBuf>,
    is_target: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|d| !is_excluded(d, exclude))
        .filter_map(|d| resolve(&d))
        .filter(|d| !is_excluded(d, exclude) && is_target(d))
        .collect();
    found.sort_by_key(|p| norm(&p.to_string_lossy()));
    found.dedup_by_key(|p| norm(&p.to_string_lossy()));
    let keys: Vec<String> = found.iter().map(|p| norm(&p.to_string_lossy())).collect();
    let mut out = Vec::new();
    for (p, k) in found.into_iter().zip(&keys) {
        if !keys.iter().any(|o| o != k && under(k, o)) {
            out.push(p);
        }
    }
    out
}

#[derive(Deserialize)]
struct EsRow {
    filename: String,
}

/// es.exe の JSON 出力から CACHEDIR.TAG の親フォルダーを取り出す。
pub fn parse_es_output(text: &str) -> Result<Vec<PathBuf>, String> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<EsRow> =
        serde_json::from_str(text).map_err(|e| format!("es の出力を読めません: {e}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Path::new(&r.filename).parent().map(Path::to_path_buf))
        .collect())
}

/// PATH の中の `es.exe`。
fn es_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("es.exe"))
        .find(|p| p.is_file())
}

/// WinGet が入れる場所（`%LOCALAPPDATA%\Microsoft\WinGet\Packages\voidtools.Everything.Cli_*\es.exe`）。
fn es_in_winget() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("Microsoft")
        .join("WinGet")
        .join("Packages");
    fs::read_dir(base)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("voidtools.Everything.Cli_")
        })
        .map(|e| e.path().join("es.exe"))
        .find(|p| p.is_file())
}

/// 設定 → PATH → WinGet の順に es.exe を探す。見つからなければ `es.exe`（起動時にエラーになる）。
fn es_exe(cfg: &Config) -> PathBuf {
    if !cfg.es_path.is_empty() {
        return PathBuf::from(&cfg.es_path);
    }
    es_on_path()
        .or_else(es_in_winget)
        .unwrap_or_else(|| PathBuf::from("es.exe"))
}

/// Everything で `wfn:CACHEDIR.TAG` を探す。索引外の列は使わない（Everything が固まるため）。
/// es の標準出力は ANSI コードページで日本語が化けるため、UTF-8 で書き出す `-export-json` を使う。
/// 応答がなければ（`-timeout` と、こちら側の待ち時間の両方で）エラーを返す。
/// 結果は実行ごとの一時フォルダー `run_dir`（実行の最後に Prunlet で片付ける）へ書き出す。
pub fn find_with_everything(cfg: &Config, run_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let exe = es_exe(cfg);
    fs::create_dir_all(run_dir).map_err(|e| format!("{}: {e}", run_dir.display()))?;
    let out = run_dir.join("es-result.json");
    let mut child = Command::new(&exe)
        .args(["-timeout", &cfg.es_timeout_ms.to_string(), "-export-json"])
        .arg(&out)
        .arg("wfn:CACHEDIR.TAG")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("es を起動できません ({}): {e}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_millis(cfg.es_timeout_ms + 15_000);
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                return Err("es が応答しません（待ち時間切れ）".to_string());
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    if !status.success() {
        return Err(format!(
            "es が失敗しました（終了コード {:?}）。Everything が起動していない可能性があります",
            status.code()
        ));
    }
    let bytes = fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|e| format!("es の出力が UTF-8 ではありません: {e}"))?;
    parse_es_output(&text)
}

/// `roots` の下を走査して CACHEDIR.TAG のあるフォルダーを集める。target の内側へは下りない。
pub fn find_by_walk(roots: &[PathBuf], exclude: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = stack.pop() {
        if is_excluded(&dir, exclude) {
            continue;
        }
        if dir.join("CACHEDIR.TAG").is_file() && is_target_dir(&dir) {
            out.push(dir);
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            let link = md.file_type().is_symlink()
                || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            if md.is_dir() && !link && !WALK_SKIP.contains(&name.as_str()) {
                stack.push(e.path());
            }
        }
    }
    out
}

/// 探索の結果。`candidates` は絞り込む前の件数（Everything が 0 件を返したことの警告に使う）。
pub struct Discovered {
    pub targets: Vec<PathBuf>,
    pub candidates: usize,
}

/// 探索の入口。`roots` があれば走査、なければ Everything。
pub fn discover(cfg: &Config, roots: &[PathBuf], run_dir: &Path) -> Result<Discovered, String> {
    let candidates = if roots.is_empty() {
        find_with_everything(cfg, run_dir)?
    } else {
        find_by_walk(roots, &cfg.exclude)
    };
    let count = candidates.len();
    Ok(Discovered {
        targets: select_targets(candidates, &cfg.exclude, resolve_real, is_target_dir),
        candidates: count,
    })
}

/// 拡張長の接頭辞を外す（`\\?\C:\x` → `C:\x`、`\\?\UNC\srv\share` → `\\srv\share`）。
pub fn strip_verbatim(s: &str) -> String {
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(s).to_string(),
    }
}

/// 実体のパス。存在しない・リンクそのものなら `None`（リンクは target として扱わない）。
fn resolve_real(p: &Path) -> Option<PathBuf> {
    if crate::scan::is_link(&fs::symlink_metadata(p).ok()?) {
        return None;
    }
    let real = fs::canonicalize(p).ok()?;
    Some(PathBuf::from(strip_verbatim(&real.to_string_lossy())))
}

#[cfg(test)]
mod tests;
