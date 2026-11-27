//! 削除の経路（M3・S2・N3）を、ごみ箱の判定を差し替えて確かめる。
//! 偽の Prunlet は呼び出しを記録して固定の応答を返すだけで、何も削除しない。
//! 疑似データは `%TEMP%\scopula-tests\<pid>-<名前>` に作り、テストでは削除しない。

use super::*;
use std::fs::{self, File};
use std::time::Duration;

const DAY: u64 = 86_400;
const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("scopula-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn put(path: &Path, age_secs: u64, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    let f = File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
}

/// 10日前の incremental（1000 バイト）を持つ target を作る。
fn make_target(base: &Path, name: &str) -> PathBuf {
    let t = base.join(name).join("target");
    put(&t.join("CACHEDIR.TAG"), 30 * DAY, CARGO_TAG.as_bytes());
    put(&t.join("debug/.cargo-lock"), 10 * DAY, b"");
    put(&t.join("debug/incremental/x/s.bin"), 10 * DAY, &[0u8; 1000]);
    t
}

fn disabled(_: &Path) -> trash::Trash {
    trash::Trash::Disabled
}
fn enabled(_: &Path) -> trash::Trash {
    trash::Trash::Enabled
}
fn unknown(_: &Path) -> trash::Trash {
    trash::Trash::Unknown("テスト".to_string())
}

/// 偽の Prunlet（cmd）を作り、設定を返す。`body` は呼び出しを記録した後に実行する cmd の本文。
fn fake_cfg(base: &Path, body: &str, timeout: u64) -> (Config, PathBuf) {
    let calls = base.join("calls.txt");
    let fake = base.join("fake.cmd");
    fs::write(
        &fake,
        format!(
            "@echo off\r\necho %* >> \"{}\"\r\n{body}\r\n",
            calls.display()
        ),
    )
    .unwrap();
    let cfg = Config {
        prunlet_exe: fake.display().to_string(),
        prunlet_config: "fake-config.json".to_string(),
        prunlet_timeout_secs: timeout,
        ..Config::default()
    };
    (cfg, calls)
}

fn run_all(
    base: &Path,
    cfg: &Config,
    targets: &[PathBuf],
    trash_of: fn(&Path) -> trash::Trash,
) -> (Vec<TargetReport>, Option<String>) {
    let logger = Logger::new(base.join("logs"), "t".to_string());
    let ctx = Ctx {
        cfg,
        limits: cfg.limits(),
        dry_run: false,
        run_dir: base.join("run"),
        logger: &logger,
        trash_of,
    };
    process_all(targets, &ctx)
}

fn real(p: &Path) -> String {
    discover::strip_verbatim(&fs::canonicalize(p).unwrap().to_string_lossy())
}

#[test]
fn ごみ箱が無効なら削除へ進み_状態ごとに数える() {
    let b = fixture("main-run");
    let t = make_target(&b, "a");
    let dir = real(&t.join("debug/incremental/x"));
    let response = serde_json::json!({"items": [{"kind": "directory", "path": dir, "status": "permanently_deleted"}]});
    fs::write(b.join("response.json"), response.to_string()).unwrap();
    let body = format!("type \"{}\"", b.join("response.json").display());
    let (cfg, calls) = fake_cfg(&b, &body, 600);
    let (reports, aborted) = run_all(&b, &cfg, &[t], disabled);
    assert!(aborted.is_none());
    let r = &reports[0];
    assert!(r.skipped.is_none(), "{:?}", r.skipped);
    assert_eq!(r.deleted_bytes, 1000);
    assert_eq!(r.statuses.permanently_deleted, 1);
    assert_eq!(r.statuses.recycled, 0);
    assert!(r.warnings.is_empty());
    let recorded = fs::read_to_string(calls).unwrap();
    assert!(
        recorded.contains("delete --recursive --paths-file"),
        "{recorded}"
    );
}

#[test]
fn ごみ箱が有効または不明なら削除せず見送る() {
    for (name, f) in [("enabled", enabled as fn(&Path) -> _), ("unknown", unknown)] {
        let b = fixture(&format!("main-trash-{name}"));
        let t = make_target(&b, "a");
        let (cfg, calls) = fake_cfg(&b, "echo {}", 600);
        let (reports, aborted) = run_all(&b, &cfg, &[t], f);
        assert!(aborted.is_none());
        let why = reports[0].skipped.as_deref().unwrap();
        assert!(why.contains("元に戻せるか"), "{name}: {why}");
        assert_eq!(reports[0].deleted_bytes, 0);
        assert!(!calls.exists(), "{name}: Prunlet を呼ばない");
    }
}

#[test]
fn ごみ箱が無効なのにrecycledが返れば警告する() {
    let b = fixture("main-recycled");
    let t = make_target(&b, "a");
    let dir = real(&t.join("debug/incremental/x"));
    let response =
        serde_json::json!({"items": [{"kind": "directory", "path": dir, "status": "recycled"}]});
    fs::write(b.join("response.json"), response.to_string()).unwrap();
    let body = format!("type \"{}\"", b.join("response.json").display());
    let (cfg, _) = fake_cfg(&b, &body, 600);
    let (reports, _) = run_all(&b, &cfg, &[t], disabled);
    assert_eq!(reports[0].statuses.recycled, 1);
    assert_eq!(reports[0].warnings.len(), 1);
}

#[test]
fn prunletが固まったら打ち切り_ambiguousで実行全体を止め_次のtargetへ進まない() {
    let b = fixture("main-abort");
    let t1 = make_target(&b, "a");
    let t2 = make_target(&b, "b");
    let (cfg, calls) = fake_cfg(&b, "ping -n 6 127.0.0.1 >nul", 1);
    let (reports, aborted) = run_all(&b, &cfg, &[t1.clone(), t2.clone()], disabled);
    assert_eq!(reports.len(), 1, "2つ目の target は処理しない");
    assert_eq!(reports[0].statuses.ambiguous, 1);
    assert_eq!(reports[0].deleted_bytes, 0);
    assert!(aborted.unwrap().contains("残り 1 target"));
    assert_eq!(
        fs::read_to_string(calls)
            .unwrap()
            .matches("--paths-file")
            .count(),
        1,
        "再送しない"
    );
    // 中断後はロックが解放されている（2つ目はもちろん、1つ目の P も取れる）。
    assert!(scan::acquire_locks(&[t2.join("debug/.cargo-lock")]).is_ok());
    assert!(scan::acquire_locks(&[t1.join("debug/.cargo-lock")]).is_ok());
}

#[test]
fn pが1つも無いtargetは走査もr4もせず見送る() {
    let b = fixture("main-nop");
    let t = b.join("cache/target");
    put(&t.join("CACHEDIR.TAG"), 30 * DAY, CARGO_TAG.as_bytes());
    put(&t.join("tmp/old/f"), 10 * DAY, b"x");
    let (cfg, calls) = fake_cfg(&b, "echo {}", 600);
    let (reports, aborted) = run_all(&b, &cfg, &[t], disabled);
    assert!(aborted.is_none());
    assert_eq!(reports[0].skipped.as_deref(), Some("P なし"));
    assert!(reports[0].items.is_empty());
    assert!(!calls.exists());
}

#[test]
fn 中断で残った古い実行用フォルダーだけを片付け対象にする() {
    use std::os::windows::fs::OpenOptionsExt;
    let b = fixture("main-stale");
    let parent = b.join("scopula");
    let (old, young, current) = (
        parent.join("old"),
        parent.join("young"),
        parent.join("current"),
    );
    for d in [&old, &young, &current] {
        fs::create_dir_all(d).unwrap();
    }
    put(&parent.join("file.txt"), 3 * DAY, b"x"); // フォルダーでないものは対象外
    // フォルダーの更新時刻は、バックアップ用のフラグで開いて変える。
    let handle = File::options()
        .write(true)
        .custom_flags(0x0200_0000)
        .open(&old)
        .unwrap();
    handle
        .set_modified(SystemTime::now() - Duration::from_secs(2 * DAY))
        .unwrap();
    drop(handle);
    assert_eq!(prune::stale_run_dirs(&parent, &current), [old]);
}
