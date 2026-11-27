//! R1〜R6 の選別を、一時フォルダーに作った疑似 target（実ファイルと更新時刻）で確かめる。
//! 疑似 target は `%TEMP%\scopula-tests\<pid>-<名前>` に作り、テストでは削除しない（後で Prunlet で消す）。

use super::*;
use crate::scan::{
    ScanErrors, fingerprint_hex, is_cargo_tag, parse_print, read_profile_units, scan_dir,
};
use std::fs::{self, File};
use std::path::Path;
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

/// `rel` に内容つきのファイルを作り、更新時刻を `age_secs` 秒前にする。
fn put_str(base: &Path, rel: &str, age_secs: u64, content: &str) {
    let path = base.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    let f = File::options().write(true).open(&path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
}

fn put(base: &Path, rel: &str, age_secs: u64, bytes: usize) {
    put_str(base, rel, age_secs, &"0".repeat(bytes));
}

/// P を作る（`.cargo-lock` を置く）。
fn lock(base: &Path, p: &str, age_secs: u64) {
    put(base, &format!("{p}/.cargo-lock"), age_secs, 0);
}

/// 新配置のユニット `P\build\<pkg>\<hash>` を作る。`print` は自身の指紋値、`deps` は依存先の指紋値。
fn unit(base: &Path, p: &str, pkg: &str, hash: &str, print: u64, deps: &[u64], age: u64) {
    let dir = format!("{p}/build/{pkg}/{hash}");
    let deps_json: Vec<String> = deps
        .iter()
        .map(|d| format!("[1,\"x\",false,{d}]"))
        .collect();
    let json = format!("{{\"deps\":[{}]}}", deps_json.join(","));
    put_str(
        base,
        &format!("{dir}/fingerprint/lib-{pkg}"),
        age,
        &fingerprint_hex(print),
    );
    put_str(
        base,
        &format!("{dir}/fingerprint/lib-{pkg}.json"),
        age,
        &json,
    );
    put(
        base,
        &format!("{dir}/fingerprint/invoked.timestamp"),
        age,
        0,
    );
    put(base, &format!("{dir}/out/o"), age, 10);
}

fn limits(cap_bytes: u64) -> Limits {
    Limits {
        cap_bytes,
        ..Limits::default()
    }
}

fn run(base: &Path, cap_bytes: u64) -> PlanResult {
    let mut errors = ScanErrors::default();
    let root = scan_dir(base, 0, u32::MAX, &mut errors);
    assert_eq!(errors.count, 0);
    let units: Vec<ProfileUnits> = profile_dirs(&root)
        .into_iter()
        .map(read_profile_units)
        .collect();
    plan_target(&root, &units, SystemTime::now(), &limits(cap_bytes))
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// 計画を「規則 相対パス」の並びにして返す。
fn names(base: &Path, cap_bytes: u64) -> Vec<String> {
    let mut v: Vec<String> = run(base, cap_bytes)
        .items
        .iter()
        .map(|i| format!("{:?} {}", i.rule, rel(base, &i.path)))
        .collect();
    v.sort();
    v
}

// ---- B1: target と P の判定 ----

#[test]
fn cargoのtagだけをtargetのしるしとみなす() {
    assert!(is_cargo_tag(CARGO_TAG.as_bytes()));
    // uv の文面（署名は同じでも作成元が違う）・署名だけ・作成元だけは cargo 製ではない。
    let uv = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by uv.\n";
    assert!(!is_cargo_tag(uv.as_bytes()));
    assert!(!is_cargo_tag(
        b"Signature: 8a477f597d28d172789f06886806bc55\n"
    ));
    assert!(!is_cargo_tag(b"# created by cargo\n"));
}

#[test]
fn pは_cargo_lockを持つフォルダーで_triple下も判定し_保護名は除く() {
    let b = fixture("b1");
    lock(&b, "debug", DAY);
    put(&b, "Debug2/x", DAY, 1); // MSBuild の出力のように .cargo-lock が無いものは P でない
    lock(&b, "tmp", DAY); // 保護名
    put_str(&b, "x86_64-pc-windows-msvc/CACHEDIR.TAG", DAY, CARGO_TAG);
    lock(&b, "x86_64-pc-windows-msvc/release", DAY);
    lock(&b, "other-dir/release", DAY); // cargo 製の TAG が無い X の子は P でない
    let mut errors = ScanErrors::default();
    let root = scan_dir(&b, 0, u32::MAX, &mut errors);
    let mut ps: Vec<String> = profile_dirs(&root)
        .iter()
        .map(|n| rel(&b, &n.path))
        .collect();
    ps.sort();
    assert_eq!(ps, ["debug", "x86_64-pc-windows-msvc/release"]);
    // ロックの候補は、全 P の3種類。保護名でも .cargo-lock があれば含める（S1）。
    assert_eq!(lock_paths(&root).len(), 9);
}

// ---- M1: 指紋の依存たどり ----

#[test]
fn 指紋値の変換はリトルエンディアンの16進() {
    assert_eq!(fingerprint_hex(12809384588755960660), "5483299c6a13c4b1");
    assert_eq!(parse_print("5483299c6a13c4b1"), Some(12809384588755960660));
    assert_eq!(parse_print("5483299c6a13c4b"), None);
    assert_eq!(parse_print("zz83299c6a13c4b1"), None);
}

#[test]
fn r3_起点から届く古いユニットは残し_届かない古いユニットを消す() {
    let b = fixture("r3a");
    lock(&b, "debug", DAY);
    unit(&b, "debug", "app", "aaaaaaaaaaaaaaa1", 1, &[2], DAY); // 起点（最終ビルド）
    unit(&b, "debug", "dep", "aaaaaaaaaaaaaaa2", 2, &[3], 20 * DAY); // 起点から届く
    unit(&b, "debug", "leaf", "aaaaaaaaaaaaaaa3", 3, &[], 20 * DAY); // 推移的に届く
    unit(&b, "debug", "old", "aaaaaaaaaaaaaaa4", 4, &[], 20 * DAY); // 届かない古い
    unit(&b, "debug", "young", "aaaaaaaaaaaaaaa5", 5, &[], 3 * DAY); // 届かないが7日未満
    unit(
        &b,
        "debug",
        "ghost",
        "aaaaaaaaaaaaaaa6",
        6,
        &[999],
        20 * DAY,
    ); // 古い・参照先が無くても無視
    assert_eq!(
        names(&b, u64::MAX),
        [
            "R3 debug/build/ghost/aaaaaaaaaaaaaaa6",
            "R3 debug/build/old/aaaaaaaaaaaaaaa4"
        ]
    );
}

#[test]
fn r3_pをまたぐ参照は使用中として残す() {
    let b = fixture("r3b");
    put_str(&b, "x86_64-pc-windows-msvc/CACHEDIR.TAG", DAY, CARGO_TAG);
    lock(&b, "debug", DAY);
    lock(&b, "x86_64-pc-windows-msvc/debug", DAY);
    unit(
        &b,
        "x86_64-pc-windows-msvc/debug",
        "app",
        "bbbbbbbbbbbbbbb1",
        1,
        &[2],
        DAY,
    );
    // host 側の proc-macro。target 側の起点から届くので古くても残す。
    unit(&b, "debug", "macro", "bbbbbbbbbbbbbbb2", 2, &[], 30 * DAY);
    unit(&b, "debug", "unused", "bbbbbbbbbbbbbbb3", 3, &[], 30 * DAY);
    unit(
        &b,
        "debug",
        "host-app",
        "bbbbbbbbbbbbbbb4",
        4,
        &[],
        10 * DAY,
    );
    // debug 自身の起点（10 日前の前の1週間）からは届かないが、triple 側から届く macro は残る。
    assert_eq!(
        names(&b, u64::MAX),
        ["R3 debug/build/unused/bbbbbbbbbbbbbbb3"]
    );
}

#[test]
fn r3_最終ビルドより前の1週間は起点として残す() {
    let b = fixture("r3c");
    lock(&b, "debug", 20 * DAY);
    unit(&b, "debug", "x", "ccccccccccccccc1", 1, &[], 20 * DAY); // 最終ビルド
    unit(&b, "debug", "y", "ccccccccccccccc2", 2, &[], 25 * DAY); // 最終ビルドの前の1週間
    unit(&b, "debug", "z", "ccccccccccccccc3", 3, &[], 30 * DAY); // それより前
    assert_eq!(names(&b, u64::MAX), ["R3 debug/build/z/ccccccccccccccc3"]);
}

#[test]
fn 指紋の無いユニットは7日で分かれる() {
    let b = fixture("r3d");
    lock(&b, "debug", DAY);
    unit(&b, "debug", "app", "ddddddddddddddd1", 1, &[], DAY);
    put(&b, "debug/build/e/ddddddddddddddd2/out/o", 8 * DAY, 10);
    put(&b, "debug/build/e/ddddddddddddddd3/out/o", 2 * DAY, 10);
    assert_eq!(names(&b, u64::MAX), ["R3 debug/build/e/ddddddddddddddd2"]);
}

#[test]
fn 空の指紋ファイルは壊れではなく_指紋値なしで依存だけたどる() {
    // cargo はビルドの前に指紋ファイルを空にする。ビルド失敗直後や途中はそのまま残る。
    let b = fixture("empty-print");
    lock(&b, "debug", DAY);
    let e = "debug/build/app/fffffffffffffff1/fingerprint";
    put_str(&b, &format!("{e}/bin-app"), DAY, "");
    put_str(
        &b,
        &format!("{e}/bin-app.json"),
        DAY,
        "{\"deps\":[[1,\"x\",false,2]]}",
    );
    put(&b, "debug/build/app/fffffffffffffff1/out/o", DAY, 10);
    unit(&b, "debug", "dep", "fffffffffffffff2", 2, &[], 20 * DAY); // 空の指紋のユニットから届く
    unit(&b, "debug", "old", "fffffffffffffff3", 3, &[], 20 * DAY); // 届かない
    put(&b, "debug/deps/a.rlib", 20 * DAY, 10);
    let r = run(&b, u64::MAX);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(
        names(&b, u64::MAX),
        ["R1 debug/deps", "R3 debug/build/old/fffffffffffffff3"]
    );
    // 空以外の不正な中身は、これまでどおり壊れているとして止める。
    let c = fixture("empty-print-bad");
    lock(&c, "debug", DAY);
    put_str(&c, &format!("{e}/bin-app"), DAY, "zz");
    put_str(&c, &format!("{e}/bin-app.json"), DAY, "{\"deps\":[]}");
    put(&c, "debug/build/app/fffffffffffffff1/out/o", DAY, 10);
    assert_eq!(run(&c, u64::MAX).warnings.len(), 1);
}

#[test]
fn 壊れた指紋はその_targetのr1とr3を実行せず警告する() {
    let cases = [
        ("json", "lib-x.json", "{ not json"),
        ("shape", "lib-x.json", "{\"deps\":[[1,2]]}"),
        ("nodeps", "lib-x.json", "{}"),
        ("hex", "lib-x", "zzzz"),
    ];
    for (name, file, content) in cases {
        let b = fixture(&format!("broken-{name}"));
        lock(&b, "debug", DAY);
        unit(&b, "debug", "ok", "eeeeeeeeeeeeeee1", 1, &[], DAY);
        unit(&b, "debug", "old", "eeeeeeeeeeeeeee2", 2, &[], 20 * DAY);
        put(&b, "debug/deps/a", 20 * DAY, 10);
        put_str(
            &b,
            &format!("debug/build/x/eeeeeeeeeeeeeee3/fingerprint/{file}"),
            20 * DAY,
            content,
        );
        if file != "lib-x" {
            put_str(
                &b,
                "debug/build/x/eeeeeeeeeeeeeee3/fingerprint/lib-x",
                20 * DAY,
                &fingerprint_hex(3),
            );
        } else {
            put_str(
                &b,
                "debug/build/x/eeeeeeeeeeeeeee3/fingerprint/lib-x.json",
                20 * DAY,
                "{\"deps\":[]}",
            );
        }
        let r = run(&b, u64::MAX);
        assert!(r.items.is_empty(), "{name}: {:?}", r.items);
        assert_eq!(r.warnings.len(), 1, "{name}");
    }
}

#[test]
fn r1は新配置があり旧配置全体が古いときだけ選ぶ() {
    let b = fixture("r1");
    lock(&b, "debug", DAY);
    unit(&b, "debug", "app", "1111111111111111", 1, &[], DAY);
    put(&b, "debug/deps/a.rlib", 8 * DAY, 10);
    put(&b, "debug/.fingerprint/x/f", 9 * DAY, 10);
    put(&b, "debug/build/foo-0123456789abcdef/o", 10 * DAY, 10);
    assert_eq!(
        names(&b, u64::MAX),
        [
            "R1 debug/.fingerprint",
            "R1 debug/build/foo-0123456789abcdef",
            "R1 debug/deps"
        ]
    );
    // 旧配置の一部が7日未満なら、ひとまとめで残す。
    put(&b, "debug/build/bar-0123456789abcdef/o", 2 * DAY, 10);
    assert_eq!(names(&b, u64::MAX), Vec::<String>::new());
    // 新配置が無い P は R1 の対象外。
    let c = fixture("r1-legacy-only");
    lock(&c, "debug", DAY);
    put(&c, "debug/deps/a.rlib", 20 * DAY, 10);
    assert_eq!(names(&c, u64::MAX), Vec::<String>::new());
}

#[test]
fn examplesとp直下のファイルはどの規則でも消さない() {
    let b = fixture("keep");
    lock(&b, "debug", 40 * DAY);
    put(&b, "debug/examples/e.exe", 40 * DAY, 10);
    put(&b, "debug/app.exe", 40 * DAY, 10);
    put(&b, "debug/app.pdb", 40 * DAY, 10);
    put(&b, "debug/.cargo-build-lock", 40 * DAY, 0);
    put(&b, "debug/deps/a", 40 * DAY, 10);
    put(&b, "debug/incremental/c-1/f", 40 * DAY, 10);
    put(&b, "bundle/x", 40 * DAY, 10);
    put(&b, "artifacts/y", 40 * DAY, 10);
    unit(&b, "debug", "u", "2222222222222222", 1, &[], 40 * DAY);
    // P 全体が 30 日以上更新されていないので R6。消すのは4つのフォルダーだけ。
    assert_eq!(
        names(&b, u64::MAX),
        ["R6 debug/build", "R6 debug/deps", "R6 debug/incremental"]
    );
}

#[test]
fn r6は30日で分かれる() {
    let a = fixture("r6-29");
    lock(&a, "debug", 29 * DAY);
    put(&a, "debug/deps/a", 29 * DAY, 10);
    assert_eq!(names(&a, u64::MAX), Vec::<String>::new());
    let b = fixture("r6-31");
    lock(&b, "debug", 31 * DAY);
    put(&b, "debug/deps/a", 31 * DAY, 10);
    put(&b, "debug/.fingerprint/x/f", 31 * DAY, 10);
    assert_eq!(
        names(&b, u64::MAX),
        ["R6 debug/.fingerprint", "R6 debug/deps"]
    );
}

#[test]
fn r6は他のpの起点から届くpを外し_自分の起点だけなら外さない() {
    // N1: triple だけでビルドすると host の P は書き直されず古くなるが、proc-macro は使われている。
    let b = fixture("r6-host");
    put_str(&b, "x86_64-pc-windows-msvc/CACHEDIR.TAG", DAY, CARGO_TAG);
    lock(&b, "debug", 40 * DAY);
    lock(&b, "x86_64-pc-windows-msvc/debug", DAY);
    unit(&b, "debug", "macro", "cccccccccccccc01", 2, &[], 40 * DAY);
    unit(
        &b,
        "x86_64-pc-windows-msvc/debug",
        "app",
        "cccccccccccccc02",
        1,
        &[2],
        DAY,
    );
    let got = names(&b, u64::MAX);
    assert!(!got.iter().any(|n| n.starts_with("R6")), "{got:?}");
    // 参照が無ければ、同じ古さの host の P は R6 になる。
    let c = fixture("r6-host-unused");
    put_str(&c, "x86_64-pc-windows-msvc/CACHEDIR.TAG", DAY, CARGO_TAG);
    lock(&c, "debug", 40 * DAY);
    lock(&c, "x86_64-pc-windows-msvc/debug", DAY);
    unit(&c, "debug", "macro", "cccccccccccccc01", 2, &[], 40 * DAY);
    unit(
        &c,
        "x86_64-pc-windows-msvc/debug",
        "app",
        "cccccccccccccc02",
        1,
        &[],
        DAY,
    );
    assert_eq!(names(&c, u64::MAX), ["R6 debug/build"]);
}

#[test]
fn r2とr4は7日と3日で分かれる() {
    let b = fixture("r2r4");
    lock(&b, "release", DAY);
    put(&b, "release/incremental/a-1/f", 8 * DAY, 10);
    put(&b, "release/incremental/b-2/f", 6 * DAY, 10);
    put(&b, "tmp/old/f", 4 * DAY, 10);
    put(&b, "tmp/new/f", DAY, 10);
    assert_eq!(
        names(&b, u64::MAX),
        ["R2 release/incremental/a-1", "R4 tmp/old"]
    );
}

#[test]
fn r5は上限を超えるときincrementalを先に_その後buildを選ぶ() {
    let b = fixture("r5");
    lock(&b, "release", DAY);
    put(&b, "release/incremental/c-1/f", DAY, 300);
    put(&b, "release/build/pkg/h/out/f", DAY, 500);
    put(&b, "release/app.exe", DAY, 100);
    // 合計 900。incremental(300) を消せば 600 で上限 700 以下。
    assert_eq!(names(&b, 700), ["R5 release/incremental"]);
    // 上限 0 なら incremental の後に build も選ぶ。exe は残す。
    assert_eq!(names(&b, 0), ["R5 release/build", "R5 release/incremental"]);
}

#[test]
fn r5_配下ですでに選んだ項目は上位の項目に含めて重ねない() {
    let b = fixture("r5b");
    lock(&b, "release", DAY);
    put(&b, "release/incremental/c-old/f", 10 * DAY, 200);
    put(&b, "release/incremental/c-new/f", DAY, 300);
    put(&b, "release/app.exe", DAY, 50);
    // R2 で c-old(200) を選んでも 350 で上限 100 を超える。incremental 全体（500）に置き換える。
    assert_eq!(names(&b, 100), ["R5 release/incremental"]);
}
