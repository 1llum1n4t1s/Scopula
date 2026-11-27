# Scopulaの開発

Rustの`target`フォルダーの古い成果物を定期的に掃除するCLI。利用者向けの説明は[README](README.md)。Rustの版は[`rust-toolchain.toml`](rust-toolchain.toml)、依存の解決は`Cargo.lock`に従う。

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

## 設計メモ

- `es.exe`の標準出力はANSIコードページで日本語が化けるため、`-export-json`（UTF-8）を`%TEMP%\scopula\<実行ID>\es-result.json`へ書き出して読む。`-dc`などの索引外の列は使わない（Everythingが固まる）。
- target判定は`CACHEDIR.TAG`の本文（cargo製）。P判定は`.cargo-lock`の有無で、triple下も見る。
- R3は、新配置`build\<pkg>\<16桁>`のユニットを指紋の`deps`（u64をリトルエンディアンの16進にしたものが依存先）でたどる。P の最終ビルドから`seed_days`以内を起点に推移的に使用中とし、Pをまたいで索引化する。壊れたPが1つでもあるtargetではR1・R3・R6を止める（保守側）。R6は、R6の対象にならないPの起点から届くユニットを持つPを外す（`graph::profiles_in_use`）。
- 探索結果は`fs::canonicalize`で実体のパスにそろえ（`\\?\`・`\\?\UNC\`は外す）、ジャンクション経由の重複を除く。別のtargetの内側にあるtargetは除く。
- 走査エラーが1件でもあるtargetは見送る（先頭10件を記録）。
- サイズは長さの合計で、ハードリンクを二重に数える（見込みが多めに出る）。
- 終了コード: 0正常、1掃除後も警告上限を超えるtargetがある、2実行できなかった（設定の誤り・Everythingが応答しない）、3Prunletの結果が不明で中断した。
- Pが1つも無いtarget（cargoのregistry・gitキャッシュなど、署名入りのCACHEDIR.TAGだけを持つもの）は、走査もR4もせず「P なし」で見送る。
- 中断で残った`%TEMP%\scopula\<run-id>`は、次の正常な実行（dry-run以外）で、24時間より古いものだけPrunletで消す。
- 登録（タスクスケジューラー）とインストールは別の担当が行う（`register.ps1`は置かない）。
