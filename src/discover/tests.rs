//! target の探索（B1・L2・L3）の判定を確かめる。
//! 疑似データは `%TEMP%\scopula-tests\<pid>-<名前>` に作り、テストでは削除しない。

use super::*;

const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";
const UV_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by uv.\n";

fn fixture(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("scopula-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn tag(dir: &Path, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("CACHEDIR.TAG"), text).unwrap();
}

#[test]
fn 走査はcargo製のtagを持つフォルダーだけをtargetとし_内側と除外の下は見ない() {
    let b = fixture("walk");
    tag(&b.join("a/target"), CARGO_TAG);
    tag(&b.join("a/target/debug/build/inner/target"), CARGO_TAG); // target の内側
    tag(&b.join("uv/.cache"), UV_TAG); // uv のキャッシュ
    tag(&b.join("web/node_modules/pkg/target"), CARGO_TAG);
    tag(&b.join("repo/.git/x/target"), CARGO_TAG);
    let found = find_by_walk(std::slice::from_ref(&b), &[]);
    let targets = select_targets(found, &[], |p| Some(p.to_path_buf()), is_target_dir);
    assert_eq!(targets, [b.join("a/target")]);
}

#[test]
fn 別のtargetの内側とリンクと除外は選ばない() {
    let a = PathBuf::from(r"C:\w\a\target");
    let inner = PathBuf::from(r"C:\w\a\target\debug\x\target");
    let link = PathBuf::from(r"C:\w\link\target");
    let cache = PathBuf::from(r"D:\cargo-cache\registry\x\target");
    let nm = PathBuf::from(r"C:\w\web\node_modules\p\target");
    let recycled = PathBuf::from(r"C:\$Recycle.Bin\S-1\$R1\target");
    let link_key = link.clone();
    let resolve = move |p: &Path| (p != link_key).then(|| p.to_path_buf());
    let got = select_targets(
        vec![inner, a.clone(), link, cache, nm, recycled, a.clone()],
        &[r"D:\cargo-cache".to_string()],
        resolve,
        |_| true,
    );
    assert_eq!(got, [a]);
}

#[test]
fn 拡張長の接頭辞を外す_unc対応() {
    assert_eq!(strip_verbatim(r"\\?\C:\x"), r"C:\x");
    assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\t"), r"\\srv\share\t");
    assert_eq!(strip_verbatim(r"D:\x"), r"D:\x");
}

#[test]
fn esの出力はcachedirtagの親フォルダーになる() {
    let json = r#"[{"filename":"D:\\a\\target\\CACHEDIR.TAG"},{"filename":"C:\\テスト\\target\\CACHEDIR.TAG"}]"#;
    let got = parse_es_output(json).unwrap();
    assert_eq!(
        got,
        [
            PathBuf::from(r"D:\a\target"),
            PathBuf::from(r"C:\テスト\target")
        ]
    );
    assert!(parse_es_output("").unwrap().is_empty());
    assert!(parse_es_output("x").is_err());
}
