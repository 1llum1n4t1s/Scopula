//! target の走査と、cargo の指紋（fingerprint）の読み取り、`.cargo-*lock` の保持。
//! フォルダーだけを木として保持し、ファイルは親のサイズと最新時刻に畳み込む。
//! 判定側（plan）がファイルシステムを触らずに済むよう、必要な事実はここで集める。

use std::fs::{self, File, Metadata, TryLockError};
use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 木として保持するフォルダーの深さの上限。
/// target(0)\<triple>(1)\debug(2)\build(3)\<pkg>(4)\<hash>(5) まで保持すれば判定に足りる。
const KEEP_DEPTH: u32 = 5;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// 記録する「読めなかったパス」の最大件数。
const ERROR_SAMPLES: usize = 10;
/// cargo が `CACHEDIR.TAG` に書く署名と作成元の文言。
const TAG_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
const TAG_CREATED_BY: &[u8] = b"created by cargo";

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    /// 配下の全ファイルの合計バイト数。
    pub size: u64,
    /// 配下の最新のファイル更新時刻。ファイルが無ければフォルダー自身の更新時刻。
    pub newest: SystemTime,
    /// 子フォルダー（ファイルは含まない）。
    pub children: Vec<Node>,
    /// 直下に（リンクでない）`.cargo-lock` ファイルがある。
    pub has_cargo_lock: bool,
    /// 直下に cargo 製の `CACHEDIR.TAG` がある。
    pub cargo_tag: bool,
}

impl Node {
    /// 名前（大文字小文字を区別しない）で子フォルダーを探す。
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// 最新の更新から `secs` 秒以上経っているか。未来の時刻は古いとみなさない。
    pub fn older_than(&self, now: SystemTime, secs: u64) -> bool {
        older_than(self.newest, now, secs)
    }
}

/// `t` が `now` より `secs` 秒以上前か。未来の時刻は古いとみなさない。
pub fn older_than(t: SystemTime, now: SystemTime, secs: u64) -> bool {
    now.duration_since(t)
        .map(|d| d.as_secs() >= secs)
        .unwrap_or(false)
}

/// 走査で読めなかった項目（M4）。1件でもあれば、その target は掃除しない。
#[derive(Debug, Default, Clone)]
pub struct ScanErrors {
    pub count: u64,
    /// 先頭の数件のパス。
    pub first: Vec<PathBuf>,
}

impl ScanErrors {
    fn add(&mut self, path: &Path) {
        self.count += 1;
        if self.first.len() < ERROR_SAMPLES {
            self.first.push(path.to_path_buf());
        }
    }
}

/// `CACHEDIR.TAG` の先頭の内容が cargo 製か。
pub fn is_cargo_tag(head: &[u8]) -> bool {
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    has(TAG_SIGNATURE) && has(TAG_CREATED_BY)
}

/// `CACHEDIR.TAG` の先頭 1KiB を読んで cargo 製か確かめる。
pub fn file_is_cargo_tag(path: &Path) -> bool {
    let Ok(f) = File::open(path) else {
        return false;
    };
    let mut buf = Vec::new();
    let _ = f.take(1024).read_to_end(&mut buf);
    is_cargo_tag(&buf)
}

/// リンク（シンボリックリンク・ジャンクション）か。辿らない。
pub fn is_link(md: &Metadata) -> bool {
    md.file_type().is_symlink() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// `path` を走査する。`max_depth` より深いフォルダーへは下りない（P の探索用の浅い走査に使う）。
/// 読めなかった項目は `errors` に記録する。
pub fn scan_dir(path: &Path, depth: u32, max_depth: u32, errors: &mut ScanErrors) -> Node {
    let mut size = 0u64;
    let mut newest: Option<SystemTime> = None;
    let mut children = Vec::new();
    let (mut has_cargo_lock, mut cargo_tag) = (false, false);
    match fs::read_dir(path) {
        Err(_) => errors.add(path),
        Ok(rd) => {
            for entry in rd {
                let Ok(entry) = entry else {
                    errors.add(path);
                    continue;
                };
                let Ok(md) = entry.metadata() else {
                    errors.add(&entry.path());
                    continue;
                };
                if is_link(&md) {
                    continue;
                }
                let modified = if md.is_dir() {
                    if depth >= max_depth {
                        continue;
                    }
                    let child = scan_dir(&entry.path(), depth + 1, max_depth, errors);
                    size += child.size;
                    let t = child.newest;
                    if depth < KEEP_DEPTH {
                        children.push(child);
                    }
                    Some(t)
                } else {
                    size += md.len();
                    let name = entry.file_name();
                    has_cargo_lock |= name == ".cargo-lock";
                    if name == "CACHEDIR.TAG" {
                        cargo_tag = file_is_cargo_tag(&entry.path());
                    }
                    md.modified().ok()
                };
                newest = newest.max(modified);
            }
        }
    }
    let newest = newest
        .or_else(|| fs::metadata(path).and_then(|m| m.modified()).ok())
        .unwrap_or(UNIX_EPOCH);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Node {
        name,
        path: path.to_path_buf(),
        size,
        newest,
        children,
        has_cargo_lock,
        cargo_tag,
    }
}

/// 旧配置の `build\<pkg>-<16桁の16進>` か。
pub fn is_legacy_build_name(name: &str) -> bool {
    match name.rsplit_once('-') {
        Some((pkg, hash)) => !pkg.is_empty() && is_hash16(hash),
        None => false,
    }
}

fn is_hash16(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---- cargo の指紋 ----

/// 新配置のユニット `P\build\<pkg>\<hash>\`。
#[derive(Debug, Clone)]
pub struct Unit {
    pub path: PathBuf,
    /// ユニット全体のバイト数と最新のファイル更新時刻。
    pub size: u64,
    pub newest: SystemTime,
    /// `fingerprint\` の中の最新のファイル更新時刻。指紋ファイルが無ければ `None`。
    pub fp_newest: Option<SystemTime>,
    /// このユニット自身の指紋値（指紋ファイルの中身）。
    pub prints: Vec<u64>,
    /// 依存先の指紋値（JSON の `deps[*][3]`）。
    pub deps: Vec<u64>,
}

/// P 1つ分のユニット。形式が読めなければ `broken` に理由を入れ、その P の R1・R3 は実行しない。
#[derive(Debug, Clone)]
pub struct ProfileUnits {
    pub path: PathBuf,
    pub units: Vec<Unit>,
    pub broken: Option<String>,
}

/// 指紋値（`deps[*][3]` の u64）を、指紋ファイルの中身と同じ 16 桁の 16 進にする（リトルエンディアン）。
/// 実行時は逆向き（`parse_print`）だけを使い、テストで両者の対応を確かめる。
#[cfg(test)]
pub fn fingerprint_hex(v: u64) -> String {
    v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// 指紋ファイルの中身（16 桁の 16 進）を指紋値にする。
pub fn parse_print(s: &str) -> Option<u64> {
    if !is_hash16(s) {
        return None;
    }
    let mut bytes = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(u64::from_le_bytes(bytes))
}

/// 指紋ファイル名か（拡張子なしで、`dep-`・`output-` で始まらない）。
fn is_print_name(name: &str) -> bool {
    !name.contains('.') && !name.starts_with("dep-") && !name.starts_with("output-")
}

/// 指紋の JSON から依存先の指紋値を取り出す。形が違えばエラー。
fn parse_deps(json: &str) -> Result<Vec<u64>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let deps = v
        .get("deps")
        .and_then(|d| d.as_array())
        .ok_or("deps が配列でない")?;
    deps.iter()
        .map(|d| {
            d.as_array()
                .and_then(|a| a.get(3))
                .and_then(|x| x.as_u64())
                .ok_or_else(|| "deps の要素の形が違う".to_string())
        })
        .collect()
}

fn read_unit(node: &Node) -> Result<Unit, String> {
    let fp = node.path.join("fingerprint");
    let mut unit = Unit {
        path: node.path.clone(),
        size: node.size,
        newest: node.newest,
        fp_newest: None,
        prints: Vec::new(),
        deps: Vec::new(),
    };
    let rd = match fs::read_dir(&fp) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(unit),
        Err(e) => return Err(format!("{}: {e}", fp.display())),
    };
    for entry in rd {
        let entry = entry.map_err(|e| format!("{}: {e}", fp.display()))?;
        let md = entry
            .metadata()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        unit.fp_newest = unit.fp_newest.max(md.modified().ok());
        let name = entry.file_name().to_string_lossy().into_owned();
        if !md.is_file() || !is_print_name(&name) {
            continue;
        }
        let text = fs::read_to_string(entry.path())
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        // cargo はビルドの前に指紋ファイルを空にし、成功したら書き込む。0 バイトはビルド失敗の直後や
        // 途中の状態なので壊れているとは扱わず、指紋値なし（他から参照されない）として依存だけ読む。
        // 0 バイト以外の不正な中身は、これまでどおり壊れているとして止める。
        let print = if text.is_empty() {
            None
        } else {
            Some(
                parse_print(text.trim())
                    .ok_or_else(|| format!("{}: 指紋が16桁の16進でない", entry.path().display()))?,
            )
        };
        let json_path = fp.join(format!("{name}.json"));
        let json =
            fs::read_to_string(&json_path).map_err(|e| format!("{}: {e}", json_path.display()))?;
        let deps = parse_deps(&json).map_err(|e| format!("{}: {e}", json_path.display()))?;
        unit.prints.extend(print);
        unit.deps.extend(deps);
    }
    Ok(unit)
}

/// P 1つ分のユニットを読む。新配置のフォルダー（`build\<pkg>\<16桁16進>`）だけが対象。
pub fn read_profile_units(p: &Node) -> ProfileUnits {
    let mut out = ProfileUnits {
        path: p.path.clone(),
        units: Vec::new(),
        broken: None,
    };
    let Some(build) = p.child("build") else {
        return out;
    };
    let pkgs = build
        .children
        .iter()
        .filter(|n| !is_legacy_build_name(&n.name));
    for hash in pkgs.flat_map(|pkg| pkg.children.iter().filter(|h| is_hash16(&h.name))) {
        match read_unit(hash) {
            Ok(u) => out.units.push(u),
            Err(e) => {
                out.broken.get_or_insert(e);
            }
        }
    }
    out
}

// ---- ロック（M2） ----

/// 保持している `.cargo-*lock` の集まり。破棄（エラー・panic を含む）で解放する。
#[derive(Debug)]
pub struct LockSet(Vec<File>);

impl Drop for LockSet {
    fn drop(&mut self) {
        for f in &self.0 {
            let _ = f.unlock();
        }
    }
}

/// `paths` のうち存在するものを、順に排他ロックする（cargo と同じ LockFileEx、待たない）。
/// ロックファイルが無ければ作らない。1つでも取れなければ、取れたものを解放してエラーを返す。
pub fn acquire_locks(paths: &[PathBuf]) -> Result<LockSet, String> {
    let mut sorted: Vec<&PathBuf> = paths.iter().collect();
    sorted.sort();
    let mut held = LockSet(Vec::new());
    for p in sorted {
        let f = match File::open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", p.display())),
        };
        match f.try_lock() {
            Ok(()) => held.0.push(f),
            Err(TryLockError::WouldBlock) => return Err(p.display().to_string()),
            Err(TryLockError::Error(e)) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(held)
}

#[cfg(test)]
mod tests;
