//! Scopula: Rust の `target` フォルダーに溜まる古い成果物を定期的に掃除する CLI。
//!
//! 目的:
//!   Cargo は古い成果物を消さず、target の自動 GC も無い。toolchain 更新や Cargo.lock の変更のたびに
//!   ハッシュ世代が溜まり、全ドライブで数百 GiB に達するため、定期実行で古いものだけを消す。
//!
//! 設計:
//!   - 探索は Everything（es.exe）の `wfn:CACHEDIR.TAG`。応答がなければ終了コード 2（`--root` なら走査）。
//!   - target ごとに「全 P の cargo ロックを保持 → 走査し直し → 計画 → 削除 → 解放」を順に行う。
//!     走査エラーがある・直近2時間に更新がある・ロックが取れない・ごみ箱が有効な target は見送る。
//!   - 判定は走査済みの木と指紋だけから行う（plan）。削除は Prunlet CLI に任せる（prune）。
//!     Scopula 自身は何も削除しない。ごみ箱が無効なボリュームでは削除を元に戻せない。
//!   - Prunlet の結果が不明（打ち切り）になったら、ロックを解放して実行全体を止める（終了コード 3）。
//!   - 終了コード: 0 正常 / 1 掃除後も警告上限を超える target がある / 2 実行できなかった / 3 中断。
//!   - 窓を出さない GUI サブシステム。コンソールから起動したときだけ親のコンソールへ出力する。

#![windows_subsystem = "windows"]

mod config;
mod console;
mod discover;
mod log;
mod plan;
mod prune;
mod scan;
mod trash;

#[cfg(test)]
mod tests;

use clap::{Parser, Subcommand};
use config::{Config, Limits};
use log::{Logger, fmt_size};
use plan::Rule;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

#[derive(Parser)]
#[command(
    name = "scopula",
    version,
    about = "Rust の target フォルダーの古い成果物を掃除する"
)]
struct Cli {
    /// 設定（config.toml）とログの置き場。既定は `%LOCALAPPDATA%\Scopula`。
    /// タスクスケジューラーから動かすときは絶対パスで指定する。
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// target を探して古い成果物を Prunlet で削除する。
    Sweep(SweepArgs),
}

#[derive(clap::Args)]
struct SweepArgs {
    /// 何も削除せず、削除予定だけを表示・記録する（古いログも消さない。自分の一時フォルダーは片付ける）。
    #[arg(long)]
    dry_run: bool,
    /// Everything の代わりに、このフォルダーの下を走査して target を探す（複数指定可）。
    #[arg(long, num_args = 1.., action = clap::ArgAction::Append)]
    root: Vec<PathBuf>,
    /// 結果を JSON で標準出力へ出す。
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct ItemReport {
    rule: Rule,
    path: String,
    bytes: u64,
}

#[derive(Serialize)]
struct RuleCount {
    rule: Rule,
    count: u64,
    bytes: u64,
}

/// Prunlet が返した状態ごとの件数。
#[derive(Serialize, Default, Clone)]
struct Statuses {
    recycled: u64,
    permanently_deleted: u64,
    already_absent: u64,
    failed: u64,
    ambiguous: u64,
    other: u64,
}

impl Statuses {
    fn add(&mut self, other: &Statuses) {
        self.recycled += other.recycled;
        self.permanently_deleted += other.permanently_deleted;
        self.already_absent += other.already_absent;
        self.failed += other.failed;
        self.ambiguous += other.ambiguous;
        self.other += other.other;
    }

    fn of(results: &[prune::ItemResult]) -> Statuses {
        let mut s = Statuses::default();
        for r in results {
            match r.status.as_str() {
                "recycled" => s.recycled += 1,
                "permanently_deleted" => s.permanently_deleted += 1,
                "already_absent" => s.already_absent += 1,
                "failed" => s.failed += 1,
                "ambiguous" => s.ambiguous += 1,
                _ => s.other += 1,
            }
        }
        s
    }
}

#[derive(Serialize)]
struct TargetReport {
    path: String,
    total_bytes: u64,
    /// 掃除しなかった理由。
    skipped: Option<String>,
    planned_bytes: u64,
    /// 実行では Prunlet が削除済みと返した量、`--dry-run` では削除予定量。
    deleted_bytes: u64,
    remaining_bytes: u64,
    scan_errors: u64,
    /// 読めなかったパスの先頭の数件。
    unreadable: Vec<String>,
    warnings: Vec<String>,
    statuses: Statuses,
    rules: Vec<RuleCount>,
    items: Vec<ItemReport>,
}

#[derive(Serialize)]
struct Summary {
    run_id: String,
    dry_run: bool,
    total_bytes: u64,
    planned_bytes: u64,
    deleted_bytes: u64,
    statuses: Statuses,
    over_warn: Vec<String>,
    /// 実行を途中で止めた理由（Prunlet の結果が不明になったとき）。
    aborted: Option<String>,
    targets: Vec<TargetReport>,
}

/// 結果照合用に、小文字・バックスラッシュ・拡張長接頭辞なしへそろえる。
fn key(p: &str) -> String {
    discover::strip_verbatim(p)
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn empty_report(path: &Path, total: u64) -> TargetReport {
    TargetReport {
        path: path.display().to_string(),
        total_bytes: total,
        skipped: None,
        planned_bytes: 0,
        deleted_bytes: 0,
        remaining_bytes: total,
        scan_errors: 0,
        unreadable: Vec::new(),
        warnings: Vec::new(),
        statuses: Statuses::default(),
        rules: Vec::new(),
        items: Vec::new(),
    }
}

fn skipped(path: &Path, total: u64, why: String) -> TargetReport {
    TargetReport {
        skipped: Some(why),
        ..empty_report(path, total)
    }
}

fn rule_counts(items: &[plan::Item]) -> Vec<RuleCount> {
    [Rule::R1, Rule::R2, Rule::R3, Rule::R4, Rule::R5, Rule::R6]
        .into_iter()
        .filter_map(|rule| {
            let of: Vec<&plan::Item> = items.iter().filter(|i| i.rule == rule).collect();
            (!of.is_empty()).then(|| RuleCount {
                rule,
                count: of.len() as u64,
                bytes: of.iter().map(|i| i.bytes).sum(),
            })
        })
        .collect()
}

/// 実行全体で共有するもの。
struct Ctx<'a> {
    cfg: &'a Config,
    limits: Limits,
    dry_run: bool,
    run_dir: PathBuf,
    logger: &'a Logger,
    /// ボリュームのごみ箱の判定（テストで差し替える。本番は `trash::volume_trash`）。
    trash_of: fn(&Path) -> trash::Trash,
}

/// target 1つ分: ロック保持 → 走査し直し → 計画 → 削除 → 解放。
/// ロックは関数の終わり（エラー・panic を含む）で解放される。
fn process_target(idx: usize, path: &Path, ctx: &Ctx) -> TargetReport {
    // P を見つけるための浅い走査（サイズは要らない）で、全 P のロックを取る。
    let shallow = scan::scan_dir(path, 0, 2, &mut scan::ScanErrors::default());
    if plan::profile_dirs(&shallow).is_empty() {
        // cargo の registry・git キャッシュなど、P を持たない target は走査も R4 もしない。
        return skipped(path, 0, "P なし".to_string());
    }
    let _locks = match scan::acquire_locks(&plan::lock_paths(&shallow)) {
        Ok(l) => l,
        Err(who) => return skipped(path, 0, format!("cargo が使用中 ({who})")),
    };
    let now = SystemTime::now();
    // ロックを持ったまま走査し直す（走査してからロックするまでの間の変更を取りこぼさない）。
    let mut errors = scan::ScanErrors::default();
    let root = scan::scan_dir(path, 0, u32::MAX, &mut errors);
    if errors.count > 0 {
        let mut r = skipped(path, root.size, format!("走査エラー {} 件", errors.count));
        r.scan_errors = errors.count;
        r.unreadable = errors
            .first
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        return r;
    }
    if plan::is_recent(&root, now, &ctx.limits) {
        let why = format!("直近{}時間以内に更新あり", ctx.limits.recent_secs / 3600);
        return skipped(path, root.size, why);
    }
    let trash = (ctx.trash_of)(path);
    if !trash.may_delete() {
        return skipped(
            path,
            root.size,
            format!("{}（元に戻せるか確認できないため見送り）", trash.describe()),
        );
    }
    let units: Vec<scan::ProfileUnits> = plan::profile_dirs(&root)
        .into_iter()
        .map(scan::read_profile_units)
        .collect();
    let planned = plan::plan_target(&root, &units, now, &ctx.limits);
    let mut r = empty_report(path, root.size);
    r.warnings = planned.warnings;
    r.rules = rule_counts(&planned.items);
    r.planned_bytes = planned.items.iter().map(|i| i.bytes).sum();
    r.items = planned
        .items
        .iter()
        .map(|i| ItemReport {
            rule: i.rule,
            path: i.path.display().to_string(),
            bytes: i.bytes,
        })
        .collect();
    if ctx.dry_run {
        r.deleted_bytes = r.planned_bytes;
    } else if !planned.items.is_empty() {
        apply(idx, &mut r, &trash, ctx);
    }
    r.remaining_bytes = r.total_bytes.saturating_sub(r.deleted_bytes);
    r
}

/// 計画した項目を Prunlet で削除し、結果を削除量と状態ごとの件数へ反映する（ロック保持中）。
fn apply(idx: usize, r: &mut TargetReport, trash: &trash::Trash, ctx: &Ctx) {
    let paths: Vec<PathBuf> = r.items.iter().map(|i| PathBuf::from(&i.path)).collect();
    let results = prune::delete_paths(ctx.cfg, &ctx.run_dir, &idx.to_string(), &paths, ctx.logger);
    let status: HashMap<String, &str> = results
        .iter()
        .map(|x| (key(&x.path), x.status.as_str()))
        .collect();
    r.deleted_bytes = r
        .items
        .iter()
        .filter(|i| {
            status
                .get(&key(&i.path))
                .is_some_and(|s| prune::is_deleted(s))
        })
        .map(|i| i.bytes)
        .sum();
    r.statuses = Statuses::of(&results);
    if trash.may_delete() && r.statuses.recycled > 0 {
        r.warnings.push(format!(
            "ごみ箱が無効のはずが recycled が {} 件ありました",
            r.statuses.recycled
        ));
    }
}

fn log_target(logger: &Logger, r: &TargetReport, dry_run: bool, warn: u64) {
    let verb = if dry_run { "削除予定" } else { "削除" };
    let reason = r
        .skipped
        .as_deref()
        .map(|s| format!(" 見送り={s}"))
        .unwrap_or_default();
    let over = if r.remaining_bytes > warn {
        " 警告超過"
    } else {
        ""
    };
    logger.line(&format!(
        "target {} 合計={} {verb}={} 残り={} 項目={}{reason}{over}",
        r.path,
        fmt_size(r.total_bytes),
        fmt_size(r.deleted_bytes),
        fmt_size(r.remaining_bytes),
        r.items.len(),
    ));
    for w in &r.warnings {
        logger.line(&format!("  警告: {w}"));
    }
    for p in &r.unreadable {
        logger.line(&format!("  読めなかったパス: {p}"));
    }
    if !dry_run && r.skipped.is_none() {
        let s = &r.statuses;
        logger.line(&format!(
            "  結果: recycled={} permanently_deleted={} already_absent={} failed={} ambiguous={} other={}",
            s.recycled, s.permanently_deleted, s.already_absent, s.failed, s.ambiguous, s.other
        ));
    }
    // dry-run は項目の一覧を標準出力（--json）で見られるので、ログには残さない。
    for i in r.items.iter().filter(|_| !dry_run) {
        logger.line(&format!("  {:?} {} {}", i.rule, fmt_size(i.bytes), i.path));
    }
}

fn sweep(args: &SweepArgs, home_override: Option<&Path>) -> ExitCode {
    let home = config::home_dir(home_override);
    let cfg = match Config::load(&home.join("config.toml")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("設定を読めません: {e}");
            return ExitCode::from(2);
        }
    };
    let run_id = log::new_run_id();
    let logger = Logger::new(home.join("logs"), run_id.clone());
    let run_dir = std::env::temp_dir().join("scopula").join(&run_id);
    let mode = if args.dry_run { "dry-run" } else { "実行" };
    logger.line(&format!("開始 ({mode})"));
    for w in cfg.warnings() {
        logger.line(&format!("警告: {w}"));
        eprintln!("警告: {w}");
    }
    let (code, aborted) = run_sweep(args, &cfg, &logger, &run_dir, &run_id);
    if aborted {
        // Prunlet が固まっている可能性があるので、後片付けのために再び呼ばない。
        logger.line(&format!(
            "中断のため一時フォルダーを残します: {}",
            run_dir.display()
        ));
        return code;
    }
    // 実行用の一時フォルダーと（dry-run 以外は）保持期間を過ぎたログを、Prunlet で1回だけ片付ける。
    let mut expired = Vec::new();
    if !args.dry_run {
        expired = logger.expired_logs(cfg.log_keep_days);
        // 中断で残った前回までの一時フォルダー（24時間より古いもの）も、同じ呼び出しで片付ける。
        if let Some(parent) = run_dir.parent() {
            expired.extend(prune::stale_run_dirs(parent, &run_dir));
        }
    }
    if run_dir.exists() || !expired.is_empty() {
        prune::cleanup_run_dir(&cfg, &run_dir, &expired, &logger);
    }
    code
}

/// target を順に処理する。Prunlet の結果が不明（`ambiguous`）になったら、そこで止めて理由を返す。
fn process_all(targets: &[PathBuf], ctx: &Ctx) -> (Vec<TargetReport>, Option<String>) {
    let mut reports = Vec::new();
    for (idx, t) in targets.iter().enumerate() {
        let r = process_target(idx, t, ctx);
        let stop = r.statuses.ambiguous > 0;
        reports.push(r);
        if stop {
            let rest = targets.len() - idx - 1;
            let why = format!(
                "Prunlet の結果が不明のため中断しました（{} で {} 件、残り {rest} target は未処理）",
                targets[idx].display(),
                reports[idx].statuses.ambiguous
            );
            return (reports, Some(why));
        }
    }
    (reports, None)
}

/// 返す値は終了コードと、中断したか。
fn run_sweep(
    args: &SweepArgs,
    cfg: &Config,
    logger: &Logger,
    run_dir: &Path,
    run_id: &str,
) -> (ExitCode, bool) {
    let found = match discover::discover(cfg, &args.root, run_dir) {
        Ok(t) => t,
        Err(e) => {
            logger.line(&format!("target を探せません: {e}"));
            eprintln!("target を探せません: {e}");
            return (ExitCode::from(2), false);
        }
    };
    if args.root.is_empty() && found.candidates == 0 {
        logger.line("警告: Everything の結果が 0 件です（索引が空か、検索が失敗している可能性）");
    }
    logger.line(&format!("target {} 件", found.targets.len()));
    let ctx = Ctx {
        cfg,
        limits: cfg.limits(),
        dry_run: args.dry_run,
        run_dir: run_dir.to_path_buf(),
        logger,
        trash_of: trash::volume_trash,
    };
    let warn = cfg.warn_bytes();
    let (reports, aborted) = process_all(&found.targets, &ctx);
    for r in &reports {
        log_target(logger, r, args.dry_run, warn);
    }
    if let Some(why) = &aborted {
        logger.line(&format!("中断: {why}"));
        eprintln!("中断: {why}");
    }
    let mut statuses = Statuses::default();
    reports.iter().for_each(|r| statuses.add(&r.statuses));
    let over_warn: Vec<String> = reports
        .iter()
        .filter(|r| r.remaining_bytes > warn)
        .map(|r| r.path.clone())
        .collect();
    let summary = Summary {
        run_id: run_id.to_string(),
        dry_run: args.dry_run,
        total_bytes: reports.iter().map(|r| r.total_bytes).sum(),
        planned_bytes: reports.iter().map(|r| r.planned_bytes).sum(),
        deleted_bytes: reports.iter().map(|r| r.deleted_bytes).sum(),
        statuses,
        over_warn,
        aborted: aborted.clone(),
        targets: reports,
    };
    logger.line(&format!(
        "終了 target={} 合計={} 削除{}={} recycled={} permanently_deleted={} failed={} ambiguous={} 警告超過={}",
        summary.targets.len(),
        fmt_size(summary.total_bytes),
        if args.dry_run { "予定" } else { "" },
        fmt_size(summary.deleted_bytes),
        summary.statuses.recycled,
        summary.statuses.permanently_deleted,
        summary.statuses.failed,
        summary.statuses.ambiguous,
        summary.over_warn.len()
    ));
    print_summary(&summary, args.json);
    let code = if aborted.is_some() {
        ExitCode::from(3)
    } else if summary.over_warn.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    };
    (code, aborted.is_some())
}

fn rule_line(r: &TargetReport) -> String {
    r.rules
        .iter()
        .map(|c| format!("{:?}={}件/{}", c.rule, c.count, fmt_size(c.bytes)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_summary(s: &Summary, json: bool) {
    if json {
        match serde_json::to_string_pretty(s) {
            Ok(t) => println!("{t}"),
            Err(e) => eprintln!("JSON を作れません: {e}"),
        }
        return;
    }
    let verb = if s.dry_run { "削除予定" } else { "削除" };
    for r in &s.targets {
        let reason = r
            .skipped
            .as_deref()
            .map(|x| format!("  [見送り: {x}]"))
            .unwrap_or_default();
        println!(
            "{:>10}  {verb} {:>10}  {}{reason}",
            fmt_size(r.total_bytes),
            fmt_size(r.deleted_bytes),
            r.path
        );
        let detail = rule_line(r);
        if !detail.is_empty() {
            println!("{:>10}  {detail}", "");
        }
        for w in &r.warnings {
            println!("{:>10}  警告: {w}", "");
        }
    }
    println!(
        "合計 {}  {verb} {}  target {} 件（サイズはハードリンクを二重に数えるため、実際に空く量より大きい）",
        fmt_size(s.total_bytes),
        fmt_size(s.deleted_bytes),
        s.targets.len()
    );
    for p in &s.over_warn {
        println!("警告: 掃除後も上限を超えています: {p}");
    }
}

fn main() -> ExitCode {
    console::attach_parent_console();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sweep(args) => sweep(&args, cli.home.as_deref()),
    }
}
