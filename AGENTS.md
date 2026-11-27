# Scopulaの開発

Rustの`target`フォルダーの古い成果物を定期的に掃除するCLI。利用者向けの説明は[README](README.md)、設計は[DESIGN.md](DESIGN.md)、変更履歴は[CHANGELOG.md](CHANGELOG.md)。Rustの版は[`rust-toolchain.toml`](rust-toolchain.toml)、依存の解決は`Cargo.lock`に従う。

## 共通の規則

- 削除はScopula自身では行わず、必ずPrunlet CLI（`src/prune.rs`）に任せる。`std::fs::remove_*`などの削除APIを足さない。Prunletの`failed`・`ambiguous`は記録するだけで再送しない。呼び出しは`prunlet_timeout_secs`（既定600）で打ち切り、`ambiguous`扱いにする。`ambiguous`が出たら実行全体を止める（`process_all`、終了コード3）。
- 削除に進むのは、ごみ箱が無効（`NukeOnDelete=1`、`NoRecycleFiles`優先）と確認できたボリュームだけ（`src/trash.rs`）。有効・判定不能は見送る。消したものは戻せない前提で作る。
- ごみ箱の判定は`Ctx.trash_of`（関数ポインタ）で差し替える。本番のバイナリに判定を無効にする設定・環境変数を足さない。削除の経路は`src/tests.rs`で`trash_of`を注入して確かめ、CLIのテストは検出した状態に合う結果を必ずassertする。
- 開発中に実データへ向けてよいのは`sweep --dry-run`だけ。実削除の経路は、偽のPrunlet（`tests/cli.rs`）で確かめる。dry-runでもScopula自身の一時フォルダー（`%TEMP%\scopula\<実行ID>`）の片付けにPrunletを呼ぶ。
- 判定ロジック（`src/plan.rs`・`src/plan/graph.rs`）はファイルシステムを触らず、走査済みの木（`scan::Node`）と指紋（`scan::ProfileUnits`）だけから決める。指紋・JSONの読み取りは`scan.rs`。規則（R1〜R6）の閾値は`config::Limits`にまとめ、既定値は`config.rs`に置く。
- cargoのロック（P ごとの3種）は`scan::acquire_locks`で`File::try_lock`を使い、走査し直し・計画・削除が終わるまで保持する。ロックファイルは作らず、`share_mode(0)`で開かない（cargoを止めない）。
- 窓を出さない。`#![windows_subsystem = "windows"]`で、子プロセスは`CREATE_NO_WINDOW`で起動し、コンソールからの起動時だけ`console.rs`で出力をつなぐ。
- Cargo target・TEMP/TMP・fixtureはリポジトリ外の絶対パスへ置く。ビルドは`CARGO_TARGET_DIR=%TEMP%\scopula-build\target`（展開した絶対パス）を指定する。TEMP/TMPは上書き・未設定にしない。リポジトリ直下に`target`を作らない。
- テストの疑似データは`%TEMP%\scopula-tests\<pid>-<名前>`に作り、テストでは削除しない。溜まったら`%TEMP%\scopula-tests`をPrunlet CLIで`delete --recursive`する。
- コードコメントは日本語。cccc（`cccc <パス>`の`cognitive`）が5以下で正規表現のない関数にはテストを書かない。

## トピック別の詳細

- 探索（es.exe・target判定・走査エラー）を変える前に[docs/agents/discovery-notes.md](docs/agents/discovery-notes.md)を読む。
- 判定（R3・R6・指紋・サイズ）を変える前に[docs/agents/plan-r3-notes.md](docs/agents/plan-r3-notes.md)を読む。
- 終了コード・一時フォルダー・導入を変える前に[docs/agents/run-lifecycle-notes.md](docs/agents/run-lifecycle-notes.md)を読む。
