//! 削除は自分では行わず、Prunlet CLI に任せる。
//! パスの一覧を一時フォルダーの JSON に書き、件数上限に合わせて分割して `delete --recursive` を呼ぶ。
//! Prunlet 1回の呼び出しには時間の上限があり、超えたら打ち切って `ambiguous` として扱う（再送しない）。

use crate::config::Config;
use crate::discover::CREATE_NO_WINDOW;
use crate::log::Logger;
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

/// Prunlet の1回の削除の上限（件数・JSON のバイト数）。余裕を持たせた値。
const MAX_ITEMS: usize = 1024;
const MAX_JSON_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct ItemResult {
    pub path: String,
    pub status: String,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    items: Vec<ItemResult>,
    error: Option<String>,
}

/// Prunlet の1回の呼び出しの失敗。`ambiguous` なら削除が行われたか分からない。
#[derive(Debug, PartialEq, Eq)]
pub enum BatchError {
    /// 起動できなかった、または要求が丸ごと拒否された（何も削除していない）。
    Failed(String),
    /// 打ち切った、または結果を読めなかった（削除された可能性がある）。
    Ambiguous(String),
}

impl BatchError {
    fn status(&self) -> &'static str {
        match self {
            BatchError::Failed(_) => "failed",
            BatchError::Ambiguous(_) => "ambiguous",
        }
    }

    fn message(&self) -> &str {
        match self {
            BatchError::Failed(m) | BatchError::Ambiguous(m) => m,
        }
    }
}

/// Prunlet の JSON 結果を読む。`error` があれば全体が拒否されている。
pub fn parse_response(text: &str) -> Result<Vec<ItemResult>, BatchError> {
    let r: Response = serde_json::from_str(text)
        .map_err(|e| BatchError::Ambiguous(format!("Prunlet の出力を読めません: {e}")))?;
    match r.error {
        Some(e) => Err(BatchError::Failed(format!(
            "Prunlet が要求を拒否しました: {e}"
        ))),
        None => Ok(r.items),
    }
}

/// 削除済みとして数える状態か（すでに無かった場合を含む）。
pub fn is_deleted(status: &str) -> bool {
    matches!(
        status,
        "recycled" | "permanently_deleted" | "already_absent"
    )
}

/// 件数と JSON の大きさの上限に収まるよう、順序を保ったまま分割する。
pub fn split_batches(paths: &[PathBuf], max_items: usize, max_bytes: usize) -> Vec<Vec<PathBuf>> {
    let max_items = max_items.clamp(1, MAX_ITEMS);
    let mut out: Vec<Vec<PathBuf>> = Vec::new();
    let mut bytes = 0usize;
    for p in paths {
        // JSON では区切りと引用符、バックスラッシュの二重化の分だけ大きくなる。
        let cost = p.to_string_lossy().len() * 2 + 4;
        let start_new = out
            .last()
            .is_none_or(|b| b.len() >= max_items || bytes + cost > max_bytes);
        if start_new {
            out.push(Vec::new());
            bytes = 0;
        }
        out.last_mut().expect("直前で追加済み").push(p.clone());
        bytes += cost;
    }
    out
}

/// 子プロセスの出力を別スレッドで読み切る（読まないとパイプが詰まる）。
fn drain(mut r: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

/// `delete --recursive --paths-file <json>` を実行して標準出力（JSON）を返す。
/// 非成功の項目があると終了コードは 1 になるが、結果の JSON は読むので終了コードでは失敗にしない。
/// 上限の時間を超えたらプロセスを止めて（終了を確かめて）`Ambiguous` を返す。
fn run_prunlet(cfg: &Config, json: &Path) -> Result<String, BatchError> {
    let mut child = Command::new(&cfg.prunlet_exe)
        .arg("--config")
        .arg(&cfg.prunlet_config)
        .args(["delete", "--recursive", "--paths-file"])
        .arg(json)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            BatchError::Failed(format!(
                "Prunlet を起動できません ({}): {e}",
                cfg.prunlet_exe
            ))
        })?;
    let out = drain(child.stdout.take().expect("標準出力のパイプ"));
    let err = drain(child.stderr.take().expect("標準エラーのパイプ"));
    let deadline = Instant::now() + Duration::from_secs(cfg.prunlet_timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                // 打ち切る。終了を確かめてから戻る（呼び出し側はその後でロックを解放する）。
                let _ = child.kill();
                let _ = child.wait();
                return Err(BatchError::Ambiguous(format!(
                    "Prunlet が {} 秒で終わらず打ち切りました",
                    cfg.prunlet_timeout_secs
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(BatchError::Ambiguous(format!("Prunlet の待機に失敗: {e}"))),
        }
    };
    let text = out
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_default();
    if text.trim().is_empty() {
        let stderr = err.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
        return Err(BatchError::Ambiguous(format!(
            "Prunlet の出力が空です（終了コード {:?}）: {}",
            status.code(),
            stderr.trim()
        )));
    }
    Ok(text)
}

fn write_list(json: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let list: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let text = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    fs::write(json, text).map_err(|e| format!("{}: {e}", json.display()))
}

/// `paths` を Prunlet で削除し、項目ごとの結果を返す。`failed`・`ambiguous` は記録するだけで再送しない。
/// 失敗した分割の項目には、その状態（`failed`・`ambiguous`）を合成して返す。
/// 打ち切りが出たら、残りの分割は実行しない。
/// 一時フォルダー（`run_dir`）は呼び出し側が最後に `cleanup_run_dir` で消す。
pub fn delete_paths(
    cfg: &Config,
    run_dir: &Path,
    tag: &str,
    paths: &[PathBuf],
    log: &Logger,
) -> Vec<ItemResult> {
    let mut results = Vec::new();
    if let Err(e) = fs::create_dir_all(run_dir) {
        log.line(&format!(
            "一時フォルダーを作れません: {}: {e}",
            run_dir.display()
        ));
        return results;
    }
    for (i, batch) in split_batches(paths, cfg.prunlet_batch, MAX_JSON_BYTES)
        .iter()
        .enumerate()
    {
        let json = run_dir.join(format!("paths-{tag}-{i}.json"));
        let outcome = write_list(&json, batch)
            .map_err(BatchError::Failed)
            .and_then(|_| run_prunlet(cfg, &json))
            .and_then(|t| parse_response(&t));
        match outcome {
            Ok(items) => {
                let bad = items.iter().filter(|r| !is_deleted(&r.status)).count();
                log.line(&format!(
                    "Prunlet 分割{i}: {} 件中 非成功 {bad} 件",
                    items.len()
                ));
                for r in items
                    .iter()
                    .filter(|r| matches!(r.status.as_str(), "failed" | "ambiguous"))
                {
                    log.line(&format!("Prunlet {}: {}", r.status, r.path));
                }
                results.extend(items);
            }
            Err(e) => {
                log.line(&format!(
                    "Prunlet 分割{i} が{}（{} 件、再送しない）: {}",
                    e.status(),
                    batch.len(),
                    e.message()
                ));
                results.extend(batch.iter().map(|p| ItemResult {
                    path: p.to_string_lossy().into_owned(),
                    status: e.status().to_string(),
                }));
                if matches!(e, BatchError::Ambiguous(_)) {
                    break;
                }
            }
        }
    }
    results
}

/// 実行用の一時フォルダーと `extra`（古いログなど）を Prunlet で1回だけ消す（一覧自身も同じ削除で消える）。
/// 24時間より古い前回までの実行用フォルダー（`parent` の直下で、今回の `current` 以外）。
/// 中断したときに残るもの。削除は呼び出し側が Prunlet で行う。
pub fn stale_run_dirs(parent: &Path, current: &Path) -> Vec<PathBuf> {
    let limit = Duration::from_secs(24 * 3600);
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.path() != current)
        .filter(|e| {
            e.metadata().is_ok_and(|m| {
                m.is_dir()
                    && !crate::scan::is_link(&m)
                    && m.modified()
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > limit)
            })
        })
        .map(|e| e.path())
        .collect()
}

pub fn cleanup_run_dir(cfg: &Config, run_dir: &Path, extra: &[PathBuf], log: &Logger) {
    let mut targets: Vec<PathBuf> = vec![run_dir.to_path_buf()];
    targets.extend(extra.iter().cloned());
    let json = run_dir.join("cleanup.json");
    let result = fs::create_dir_all(run_dir)
        .map_err(|e| BatchError::Failed(e.to_string()))
        .and_then(|_| write_list(&json, &targets).map_err(BatchError::Failed))
        .and_then(|_| run_prunlet(cfg, &json))
        .and_then(|t| parse_response(&t));
    match result {
        Ok(items) => {
            for r in items {
                log.line(&format!("後片付け {}: {}", r.status, r.path));
            }
        }
        Err(e) => log.line(&format!(
            "後片付けできません: {}: {}",
            run_dir.display(),
            e.message()
        )),
    }
}
