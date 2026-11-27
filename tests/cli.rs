//! 実プロセスの `scopula sweep` を、一時フォルダーの疑似 target と偽の Prunlet で確かめる。
//! Prunlet の実物は呼ばない（偽物は呼び出しを記録して固定の JSON を返すだけで、何も削除しない）。
//! 疑似データは `%TEMP%\scopula-tests\<pid>-<名前>` に作り、テストでは削除しない。
//! ごみ箱が無効（NukeOnDelete=1）なボリュームでだけ削除に進むため、そうでない環境では
//! 削除に進む検証を見送る（その旨を標準エラーへ出す）。

use serde_json::Value;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const DAY: u64 = 86_400;
const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("scopula-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn put(base: &Path, rel: &str, age_secs: u64, content: &[u8]) {
    let path = base.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    let f = File::options().write(true).open(&path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
}

/// 10日前の incremental を持つ target（掃除される）と、直近に更新された target（見送られる）。
fn make_targets(base: &Path) {
    for name in ["old", "busy"] {
        put(
            base,
            &format!("{name}/target/CACHEDIR.TAG"),
            30 * DAY,
            CARGO_TAG.as_bytes(),
        );
        put(
            base,
            &format!("{name}/target/debug/.cargo-lock"),
            10 * DAY,
            b"",
        );
        put(
            base,
            &format!("{name}/target/debug/incremental/x/s.bin"),
            10 * DAY,
            &[0u8; 1000],
        );
    }
    put(base, "busy/target/debug/build/fresh/f", 600, b"x");
    // uv など cargo 以外の同名タグは target として扱わない。
    put(
        base,
        "old/uvcache/CACHEDIR.TAG",
        30 * DAY,
        b"Signature: 8a477f597d28d172789f06886806bc55\n# created by uv\n",
    );
    put(base, "old/uvcache/debug/deps/x", 30 * DAY, b"x");
    // 除外される .git の下。
    put(
        base,
        "old/.git/target/CACHEDIR.TAG",
        30 * DAY,
        CARGO_TAG.as_bytes(),
    );
}

fn run_sweep(base: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scopula"))
        .args(["sweep", "--json", "--home"])
        .arg(base.join("home"))
        .arg("--root")
        .arg(base.join("old"))
        .arg(base.join("busy"))
        .args(extra)
        .env("TEMP", base.join("tmp"))
        .env("TMP", base.join("tmp"))
        .output()
        .unwrap()
}

fn write_config(base: &Path, text: &str) {
    fs::create_dir_all(base.join("home")).unwrap();
    fs::create_dir_all(base.join("tmp")).unwrap();
    fs::write(base.join("home/config.toml"), text).unwrap();
}

fn parse(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn find<'a>(json: &'a Value, suffix: &str) -> &'a Value {
    json["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["path"].as_str().unwrap().ends_with(suffix))
        .unwrap()
}

/// ごみ箱の判定で見送られる環境か。
fn trash_blocks(json: &Value) -> bool {
    find(json, r"old\target")["skipped"]
        .as_str()
        .is_some_and(|s| s.contains("元に戻せるか"))
}

/// ごみ箱の判定で見送られた target の形。
fn assert_blocked(t: &Value) {
    assert!(t["skipped"].as_str().unwrap().contains("元に戻せるか"));
    assert_eq!(t["deleted_bytes"], 0);
    assert_eq!(t["planned_bytes"], 0);
}

/// 偽の Prunlet を作り、設定へ書く。`body` は cmd の本文（`%*` は引数）。
fn fake_prunlet(base: &Path, body: &str, extra_cfg: &str) -> PathBuf {
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
    write_config(
        base,
        &format!(
            "prunlet_exe = '{}'\nprunlet_config = 'fake-config.json'\n{extra_cfg}",
            fake.display()
        ),
    );
    calls
}

fn real(p: &Path) -> String {
    fs::canonicalize(p)
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

#[test]
fn dry_runは見送りと削除予定を報告し_cargo以外のタグと_gitの下は無視する() {
    let b = fixture("cli-dry");
    make_targets(&b);
    write_config(&b, "");
    put(&b, "home/logs/scopula-2020-01-01.log", 60 * DAY, b"old");
    let out = run_sweep(&b, &["--dry-run"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        b.join("home/logs/scopula-2020-01-01.log").exists(),
        "dry-run は古いログを消さない"
    );
    let json = parse(&out);
    assert_eq!(
        json["targets"].as_array().unwrap().len(),
        2,
        "uv のタグと .git の下は target ではない"
    );
    let busy = find(&json, r"busy\target");
    assert!(busy["skipped"].as_str().unwrap().contains("2時間"));
    assert_eq!(busy["planned_bytes"], 0);
    let log_dir = b.join("home/logs");
    assert!(
        fs::read_dir(log_dir).unwrap().next().is_some(),
        "ログが残る"
    );
    let old = find(&json, r"old\target");
    if trash_blocks(&json) {
        // ごみ箱が有効な環境（CI のランナーなど）: 計画に進まず見送る。
        assert_blocked(old);
        return;
    }
    assert!(old["skipped"].is_null(), "{old}");
    assert_eq!(old["planned_bytes"], 1000);
    assert_eq!(old["rules"][0]["rule"], "R2");
    assert!(b.join("old/target/debug/incremental/x/s.bin").exists());
}

#[test]
fn cargoがロック中のtargetは見送る() {
    let b = fixture("cli-lock");
    make_targets(&b);
    write_config(&b, "");
    let lock = File::open(b.join("old/target/debug/.cargo-lock")).unwrap();
    lock.try_lock().unwrap();
    let out = run_sweep(&b, &["--dry-run"]);
    drop(lock);
    let json = parse(&out);
    let old = find(&json, r"old\target");
    assert!(
        old["skipped"].as_str().unwrap().contains("cargo が使用中"),
        "{old}"
    );
    assert_eq!(old["planned_bytes"], 0);
}

#[test]
fn 実行は偽のprunletへ渡し_状態ごとに数え_一時フォルダーの削除も依頼する() {
    let b = fixture("cli-run");
    make_targets(&b);
    let dir = real(&b.join("old/target/debug/incremental/x"));
    let response = serde_json::json!({"items": [{"kind": "directory", "path": dir, "status": "permanently_deleted"}]});
    fs::write(b.join("response.json"), response.to_string()).unwrap();
    let calls = fake_prunlet(
        &b,
        &format!("type \"{}\"", b.join("response.json").display()),
        "",
    );
    let out = run_sweep(&b, &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse(&out);
    let old = find(&json, r"old\target");
    if trash_blocks(&json) {
        // 削除の経路は src/tests.rs で、判定を差し替えて確かめている。
        assert_blocked(old);
        assert!(!calls.exists(), "見送ったら Prunlet を呼ばない");
        return;
    }
    assert_eq!(old["deleted_bytes"], 1000, "{old}");
    assert_eq!(old["statuses"]["permanently_deleted"], 1);
    assert_eq!(old["statuses"]["recycled"], 0);
    let recorded = fs::read_to_string(calls).unwrap();
    assert!(
        recorded.contains("delete --recursive --paths-file"),
        "{recorded}"
    );
    assert!(recorded.contains("--config fake-config.json"));
}

#[test]
fn 応答しないprunletは打ち切って結果不明とし_再送しない() {
    let b = fixture("cli-timeout");
    make_targets(&b);
    let calls = fake_prunlet(&b, "ping -n 6 127.0.0.1 >nul", "prunlet_timeout_secs = 1\n");
    let out = run_sweep(&b, &[]);
    let json = parse(&out);
    let old = find(&json, r"old\target");
    if trash_blocks(&json) {
        assert_blocked(old);
        assert!(!calls.exists(), "見送ったら Prunlet を呼ばない");
        assert_eq!(out.status.code(), Some(0));
        return;
    }
    assert_eq!(old["statuses"]["ambiguous"], 1, "{old}");
    assert_eq!(old["deleted_bytes"], 0);
    assert_eq!(out.status.code(), Some(3), "中断は終了コード 3");
    assert!(json["aborted"].as_str().unwrap().contains("結果が不明"));
    let recorded = fs::read_to_string(calls).unwrap();
    assert_eq!(
        recorded.matches("--paths-file").count(),
        1,
        "削除は1回だけ（再送なし）。中断後は後片付けも呼ばない: {recorded}"
    );
}

/// `icacls` で Everyone の読み取りを拒否し、Drop で戻す。
struct Denied(PathBuf);

impl Denied {
    fn new(p: &Path) -> Denied {
        let st = Command::new("icacls")
            .arg(p)
            .args(["/deny", "*S-1-1-0:(R)"])
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stdout)
        );
        Denied(p.to_path_buf())
    }
}

impl Drop for Denied {
    fn drop(&mut self) {
        let _ = Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

#[test]
fn 走査で読めない項目があれば見送り_削除は0件() {
    let b = fixture("cli-unreadable");
    make_targets(&b);
    put(
        &b,
        "old/target/debug/incremental/y/locked/f",
        10 * DAY,
        b"x",
    );
    let calls = fake_prunlet(&b, "echo {}", "");
    let denied = Denied::new(&b.join("old/target/debug/incremental/y/locked"));
    let out = run_sweep(&b, &[]);
    let json = parse(&out);
    let old = find(&json, r"old\target");
    drop(denied);
    assert_eq!(old["skipped"], "走査エラー 1 件", "{old}");
    assert_eq!(old["scan_errors"], 1);
    assert_eq!(old["unreadable"].as_array().unwrap().len(), 1);
    assert_eq!(old["deleted_bytes"], 0);
    assert_eq!(old["planned_bytes"], 0);
    assert!(!calls.exists(), "Prunlet を呼ばない");
    // ACL は Denied の Drop で戻る。疑似データの片付けは Prunlet の手順に任せる。
}

#[test]
fn 警告上限を超える_targetが残れば終了コード1() {
    let b = fixture("cli-warn");
    make_targets(&b);
    write_config(&b, "warn_gib = 0\n");
    let out = run_sweep(&b, &["--dry-run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!parse(&out)["over_warn"].as_array().unwrap().is_empty());
}

#[test]
fn 設定ファイルが壊れていれば終了コード2() {
    let b = fixture("cli-badcfg");
    make_targets(&b);
    write_config(&b, "unknown_key = 1\n");
    assert_eq!(run_sweep(&b, &["--dry-run"]).status.code(), Some(2));
}
