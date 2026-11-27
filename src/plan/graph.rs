//! R3: 新配置のユニット `P\build\<pkg>\<hash>` の使用中判定（fingerprint の依存をたどる）。
//!
//! ユニットは「世代」ではなく、lib・ビルドスクリプト・その実行・features や host/target の違いごとの単位で、
//! cargo は最新のユニットを書き直さない。更新日時だけでは使用中か分からないため、
//! P の最終ビルド付近のユニットを起点に、指紋の依存を推移的にたどって届くものを使用中とみなす。

use crate::config::Limits;
use crate::scan::{ProfileUnits, Unit, older_than};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 消してよいユニットを返す。`profiles` は同じ target の全 P（triple の P は host 側 P のユニットに依存するため、
/// P をまたいで索引化する）。
///
/// 1. 起点: 各 P で、指紋の更新時刻が「その P の最終ビルド − 起点の幅」以降のユニット。
/// 2. 使用中: 起点から依存をたどって届くユニット（見つからない参照は無視）。
/// 3. 消す: 使用中でなく、指紋と全ファイルの更新時刻がどちらも最低の経過より古いもの。
///    指紋ファイルが無いユニットは、全ファイルの更新時刻だけで決める。
pub fn removable_units<'a>(
    profiles: &[&'a ProfileUnits],
    now: SystemTime,
    limits: &Limits,
) -> Vec<&'a Unit> {
    let live = vec![true; profiles.len()];
    let used = reachable(profiles, &live, limits);
    let mut out = Vec::new();
    for (pi, p) in profiles.iter().enumerate() {
        for (ui, u) in p.units.iter().enumerate() {
            let old_print = u
                .fp_newest
                .is_none_or(|t| older_than(t, now, limits.unit_secs));
            if !used.contains(&(pi, ui)) && old_print && older_than(u.newest, now, limits.unit_secs)
            {
                out.push(u);
            }
        }
    }
    out
}

/// P ごとに、`live` の P（R6 の対象にならない P）の起点から届くユニットを1つでも持つか。
/// 自分の起点から届くだけの P は、ここでは使用中にならない（自分が `live` でないため）。
pub fn profiles_in_use(profiles: &[&ProfileUnits], live: &[bool], limits: &Limits) -> Vec<bool> {
    let used = reachable(profiles, live, limits);
    (0..profiles.len())
        .map(|pi| used.iter().any(|&(p, _)| p == pi))
        .collect()
}

/// `live` の P の起点から、依存を推移的にたどって届くユニット（P の番号, ユニットの番号）。
fn reachable(
    profiles: &[&ProfileUnits],
    live: &[bool],
    limits: &Limits,
) -> HashSet<(usize, usize)> {
    let mut index: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    for (pi, p) in profiles.iter().enumerate() {
        for (ui, u) in p.units.iter().enumerate() {
            for print in &u.prints {
                index.entry(*print).or_default().push((pi, ui));
            }
        }
    }
    let seed_width = Duration::from_secs(limits.seed_secs);
    let mut used: HashSet<(usize, usize)> = HashSet::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (pi, p) in profiles.iter().enumerate() {
        if !live[pi] {
            continue;
        }
        let Some(latest) = p.units.iter().filter_map(|u| u.fp_newest).max() else {
            continue;
        };
        let floor = latest.checked_sub(seed_width).unwrap_or(UNIX_EPOCH);
        for (ui, u) in p.units.iter().enumerate() {
            if u.fp_newest.is_some_and(|t| t >= floor) && used.insert((pi, ui)) {
                stack.push((pi, ui));
            }
        }
    }
    while let Some((pi, ui)) = stack.pop() {
        for dep in &profiles[pi].units[ui].deps {
            for &next in index.get(dep).into_iter().flatten() {
                if used.insert(next) {
                    stack.push(next);
                }
            }
        }
    }
    used
}
