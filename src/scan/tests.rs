//! ロック（M2）の取り方を、実ファイルの `LockFileEx` で確かめる。
//! 疑似データは `%TEMP%\scopula-tests\<pid>-<名前>` に作り、テストでは削除しない。

use super::*;
use std::fs;

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("scopula-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    d
}

/// 別のハンドルで排他ロックを持つ（cargo がビルド中の状態）。
fn hold(path: &Path) -> File {
    fs::write(path, b"").unwrap();
    let f = File::open(path).unwrap();
    f.try_lock().unwrap();
    f
}

#[test]
fn 別のハンドルが排他ロック中なら取れず_ロック中のファイルも他者は開ける() {
    let d = fixture("lock-held");
    let p = d.join(".cargo-lock");
    let holder = hold(&p);
    let err = acquire_locks(std::slice::from_ref(&p)).unwrap_err();
    assert!(err.contains(".cargo-lock"), "{err}");
    // share_mode(0) と違い、ロック中でも開ける（cargo は待つ側になるだけ）。
    assert!(File::open(&p).is_ok());
    drop(holder);
    assert!(acquire_locks(&[p]).is_ok(), "解放後は取れる");
}

#[test]
fn ロック3つのうち1つでも取れなければ見送り_取れた分は解放される() {
    let d = fixture("lock-three");
    let names = [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];
    let paths: Vec<PathBuf> = names.iter().map(|n| d.join(n)).collect();
    for p in &paths[..2] {
        fs::write(p, b"").unwrap();
    }
    let _holder = hold(&paths[2]);
    assert!(acquire_locks(&paths).is_err());
    // 失敗した呼び出しが先に取った2つを持ち越していないこと。
    assert!(acquire_locks(&paths[..2]).is_ok());
}

#[test]
fn ロックファイルが無ければ作らず_取得中は他から取れず_破棄で解放する() {
    let d = fixture("lock-missing");
    let missing = d.join(".cargo-lock");
    let held = acquire_locks(std::slice::from_ref(&missing)).unwrap();
    assert!(!missing.exists(), "ロックファイルを作らない");
    drop(held);
    let p = d.join(".cargo-build-lock");
    fs::write(&p, b"").unwrap();
    let guard = acquire_locks(std::slice::from_ref(&p)).unwrap();
    assert!(acquire_locks(std::slice::from_ref(&p)).is_err());
    drop(guard);
    assert!(acquire_locks(&[p]).is_ok());
}
