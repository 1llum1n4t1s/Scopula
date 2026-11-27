//! 掃除対象の選別（R1〜R6）。ファイルシステムは触らず、走査済みの木（`scan::Node`）と
//! 指紋の集まり（`scan::ProfileUnits`）だけから決める。

mod graph;

use crate::config::Limits;
use crate::scan::{Node, ProfileUnits, is_legacy_build_name};
use serde::Serialize;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Rule {
    /// 旧配置（deps・.fingerprint・build\<pkg>-<hash>）。新配置があり、全体が古いときだけ。
    R1,
    /// incremental\<crate>-<hash>。
    R2,
    /// 新配置のユニット `build\<pkg>\<hash>` のうち、最終ビルドから届かず古いもの。
    R3,
    /// target\tmp\*。
    R4,
    /// target の上限超過（incremental を先に、その後 build）。
    R5,
    /// P 全体が長く更新されていないときの中間成果物。
    R6,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub rule: Rule,
    pub path: PathBuf,
    pub bytes: u64,
}

/// 計画と、実行しなかった理由などの警告。
#[derive(Debug, Default)]
pub struct PlanResult {
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
}

/// target 直下で、P としては扱わないフォルダー（消さないもの含む）。
const PROTECTED: [&str; 7] = [
    "bundle",
    "artifacts",
    "local-release",
    "tmp",
    "doc",
    "package",
    ".git",
];
/// P の中で R5・R6 が消す中間成果物のフォルダー。
const R6_DIRS: [&str; 4] = ["build", "deps", ".fingerprint", "incremental"];
/// cargo が掴むロックファイル。
const LOCK_NAMES: [&str; 3] = [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];

fn is_protected(name: &str) -> bool {
    PROTECTED.iter().any(|p| name.eq_ignore_ascii_case(p))
}

/// target 直下の P（`debug`・`release`・`<triple>\<profile>` など）を返す。
/// P は `.cargo-lock` を持つフォルダーで、target 直下か、cargo 製の `CACHEDIR.TAG` を持つ triple の子。
pub fn profile_dirs(root: &Node) -> Vec<&Node> {
    collect_profiles(root, false)
}

/// `with_protected` なら、保護名（`local-release` など）でも `.cargo-lock` を持つフォルダーを含める。
fn collect_profiles(root: &Node, with_protected: bool) -> Vec<&Node> {
    let usable = |n: &Node| with_protected || !is_protected(&n.name);
    let mut out = Vec::new();
    for c in root.children.iter().filter(|c| usable(c)) {
        if c.has_cargo_lock {
            out.push(c);
        }
        if c.cargo_tag {
            out.extend(c.children.iter().filter(|d| usable(d) && d.has_cargo_lock));
        }
    }
    out
}

/// cargo が掴むロックファイルの候補（全 P の3種類。保護名の P も `.cargo-lock` があれば含む）。
pub fn lock_paths(root: &Node) -> Vec<PathBuf> {
    collect_profiles(root, true)
        .into_iter()
        .flat_map(|p| LOCK_NAMES.iter().map(|n| p.path.join(n)))
        .collect()
}

/// 直近 `recent_secs` 以内に更新されたファイルがあるか。
pub fn is_recent(root: &Node, now: SystemTime, limits: &Limits) -> bool {
    !root.older_than(now, limits.recent_secs)
}

fn item(rule: Rule, n: &Node) -> Item {
    Item {
        rule,
        path: n.path.clone(),
        bytes: n.size,
    }
}

/// 先祖が選ばれている項目を除く（先祖に含めて数える）。
fn absorb(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out: Vec<Item> = Vec::new();
    for i in items {
        if !out.last().is_some_and(|l| i.path.starts_with(&l.path)) {
            out.push(i);
        }
    }
    out
}

/// R1: 新配置のユニットがある P で、旧配置（deps・.fingerprint・旧 build）全体が古ければまとめて選ぶ。
fn plan_legacy(p: &Node, has_units: bool, now: SystemTime, limits: &Limits, out: &mut Vec<Item>) {
    if !has_units {
        return;
    }
    let mut group: Vec<&Node> = ["deps", ".fingerprint"]
        .iter()
        .filter_map(|n| p.child(n))
        .collect();
    if let Some(build) = p.child("build") {
        group.extend(
            build
                .children
                .iter()
                .filter(|n| is_legacy_build_name(&n.name)),
        );
    }
    let newest = group.iter().map(|n| n.newest).max();
    if newest.is_some_and(|t| crate::scan::older_than(t, now, limits.legacy_secs)) {
        out.extend(group.into_iter().map(|n| item(Rule::R1, n)));
    }
}

/// R2・R6・R1（P ごと）。R6 が選ばれた P では他の規則は要らない。
/// `in_use` は、他の P の起点から届くユニットを持つこと（その P は R6 にしない）。
fn plan_profile(
    p: &Node,
    units: Option<&ProfileUnits>,
    in_use: bool,
    now: SystemTime,
    limits: &Limits,
    out: &mut Vec<Item>,
) {
    if p.older_than(now, limits.stale_profile_secs) && !in_use {
        out.extend(
            R6_DIRS
                .iter()
                .filter_map(|n| p.child(n))
                .map(|n| item(Rule::R6, n)),
        );
        return;
    }
    let readable_units = units.is_some_and(|u| u.broken.is_none() && !u.units.is_empty());
    plan_legacy(p, readable_units, now, limits, out);
    if let Some(inc) = p.child("incremental") {
        out.extend(
            inc.children
                .iter()
                .filter(|n| n.older_than(now, limits.incremental_secs))
                .map(|n| item(Rule::R2, n)),
        );
    }
}

/// R5: 上限を超えるとき、全 P の incremental を大きい順に、その後 build を大きい順に選び、上限以下で止める。
fn plan_cap(root: &Node, limits: &Limits, items: Vec<Item>) -> Vec<Item> {
    let planned = |v: &[Item]| v.iter().map(|i| i.bytes).sum::<u64>();
    let mut items = absorb(items);
    if root.size.saturating_sub(planned(&items)) <= limits.cap_bytes {
        return items;
    }
    let profiles = profile_dirs(root);
    for name in ["incremental", "build"] {
        let mut dirs: Vec<&Node> = profiles.iter().filter_map(|p| p.child(name)).collect();
        dirs.sort_by_key(|n| std::cmp::Reverse(n.size));
        for n in dirs {
            items.push(item(Rule::R5, n));
            items = absorb(items);
            if root.size.saturating_sub(planned(&items)) <= limits.cap_bytes {
                return items;
            }
        }
    }
    items
}

/// target 1つ分の削除計画（R1〜R6）。2時間条件とロックは呼び出し側で先に確かめる。
pub fn plan_target(
    root: &Node,
    units: &[ProfileUnits],
    now: SystemTime,
    limits: &Limits,
) -> PlanResult {
    let mut result = PlanResult::default();
    let profiles = profile_dirs(root);
    let mut items = Vec::new();
    let broken: Vec<&ProfileUnits> = units.iter().filter(|u| u.broken.is_some()).collect();
    for b in &broken {
        let why = b.broken.as_deref().unwrap_or_default();
        result.warnings.push(format!(
            "指紋の形式を読めないため、この target 全体で R1・R3・R6 を実行しません: {} ({why})",
            b.path.display()
        ));
    }
    let readable: Vec<&ProfileUnits> = units.iter().collect();
    // R6 の対象にならない P の起点から届く P は、R6 から外す。読めない P があれば判定できないので全部外す。
    let live: Vec<bool> = readable
        .iter()
        .map(|u| {
            profiles
                .iter()
                .find(|p| p.path == u.path)
                .is_none_or(|p| !p.older_than(now, limits.stale_profile_secs))
        })
        .collect();
    let in_use = graph::profiles_in_use(&readable, &live, limits);
    for p in &profiles {
        let u = units.iter().find(|u| u.path == p.path);
        let used = !broken.is_empty()
            || readable
                .iter()
                .position(|r| r.path == p.path)
                .is_some_and(|i| in_use[i]);
        plan_profile(p, u, used, now, limits, &mut items);
    }
    if broken.is_empty() {
        items.extend(
            graph::removable_units(&readable, now, limits)
                .into_iter()
                .map(|u| Item {
                    rule: Rule::R3,
                    path: u.path.clone(),
                    bytes: u.size,
                }),
        );
    }
    if let Some(tmp) = root.child("tmp") {
        items.extend(
            tmp.children
                .iter()
                .filter(|n| n.older_than(now, limits.tmp_secs))
                .map(|n| item(Rule::R4, n)),
        );
    }
    result.items = plan_cap(root, limits, items);
    result
}

#[cfg(test)]
mod tests;
