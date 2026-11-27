# Scopula

Rust の `target` フォルダーに溜まる古い成果物を、定期的に掃除する CLI です。Cargo は古い成果物を消さず、target の自動掃除もないため、toolchain の更新や Cargo.lock の変更のたびに世代が溜まります。Scopula は全ドライブの target を探し、使われていない古い世代だけを消します。削除は自分では行わず、Prunlet CLI に任せます。

## 使い方

```powershell
scopula sweep --dry-run          # 何も消さず、削除予定と見送り理由だけを表示
scopula sweep                    # 実際に掃除
scopula sweep --root D:\work     # Everything を使わず、指定フォルダーの下を走査（複数指定可）
scopula sweep --dry-run --json   # 結果を JSON で出力
scopula sweep --home D:\x        # 設定・ログの置き場を変える（既定は %LOCALAPPDATA%\Scopula）
```

- target は Everything の CLI（`es.exe`）で `CACHEDIR.TAG` を探して見つけます。Everything が応答しないときは終了コード 2 です（`--root` なら走査で探します）。Everything が 0 件を返したときは警告します。
- target と認めるのは、`CACHEDIR.TAG` の本文が cargo 製（署名と `created by cargo`）のフォルダーだけです。uv など他のツールの同名ファイルは対象外です。
- `.git`・`node_modules` の下、ごみ箱の中、リンク（ジャンクション等）は対象外です。cargo のホーム（registry・git）など、ほかに外したい場所は設定の `exclude` に書きます。UNC パスも扱えます。
- es.exe は、設定の `es_path` → PATH → `%LOCALAPPDATA%\Microsoft\WinGet\Packages\voidtools.Everything.Cli_*` の順に探します。
- 窓を出さない作りです（windows サブシステム、子プロセスは `CREATE_NO_WINDOW`）。タスクスケジューラーからの起動でも窓は出ません。windows サブシステムのため、シェルは終了を待たず、出力も取りこぼすことがあります。手で実行するときは `start /wait scopula sweep --dry-run` のようにするか、`scopula sweep --dry-run | more` のようにパイプへつないでください。

## 戻せるか（重要）

削除は、ごみ箱が無効なボリューム（`HKCU\...\Explorer\BitBucket\Volume\{GUID}` の `NukeOnDelete=1`、またはポリシー `NoRecycleFiles=1`）にある target に限ります。有効なボリューム、または判定できないボリュームの target は、その回は見送ります。**消したものは戻せません**（ごみ箱が無効なので永久削除になります）。結果は `recycled`・`permanently_deleted` などの状態ごとに集計し、ごみ箱が無効なのに `recycled` が返ったときは警告します。

## 何を消すか

target の下の `debug`・`release`・`<triple>\debug` など（P と呼びます。`.cargo-lock` を持つフォルダーです）ごとに判定します。

| 規則 | 対象 | 条件 |
| --- | --- | --- |
| R1 | 旧配置の `deps`・`.fingerprint`・旧 `build\<pkg>-<16桁>` をひとまとめに | 新配置（`build\<pkg>\<hash>`）が使われていて、旧配置全体の最新ファイルが7日以上前 |
| R2 | `incremental\<crate>-<hash>` | 7日以上更新がない |
| R3 | 新配置のユニット `build\<pkg>\<hash>`（後述） | 使用中でなく、7日以上更新がない |
| R4 | `target\tmp\*` | 3日以上更新がない |
| R5 | R1〜R4 の後も target が 80GiB を超えるとき、全 P の `incremental`、次に `build` | 大きい順に、上限以下になるまで |
| R6 | 30日以上更新のない P の `build`・`deps`・`.fingerprint`・`incremental` | P 全体が30日以上古い |

消さないもの: P 直下の exe・pdb・dll、`examples`、`bundle`、`artifacts`、`local-release`。

### R3 の考え方（使用中のユニットは消さない）

cargo の新配置では、世代の新旧を名前から決められません。そこで各ユニットの指紋（`build\<pkg>\<hash>\fingerprint` の中の `<16桁>.json`）から依存先をたどります。

1. P ごとの最終ビルド時刻から7日以内に更新があったユニットを起点にします。
2. 起点から、指紋の `deps` を推移的にたどって届くユニットを「使用中」とします。triple の P は host の P のユニット（ビルドスクリプト・proc-macro）に依存するため、P をまたいで索引化します。
3. 使用中でなく、指紋もファイルも7日以上古いユニットだけを R3 とします。指紋の無いユニットは7日で分けます。

指紋を読めない P が1つでもある target では、その回は R1・R3・R6 を行わず警告します。

R6 は、P ごとの最新ファイルだけでなく使用中かどうかも見ます。`--target <triple>` だけでビルドすると host 側の P（ビルドスクリプト・proc-macro だけを持つ）は書き直されず古くなりますが、R6 の対象にならない P の起点から届くユニットを1つでも持つ P は R6 から外します（自分の起点から届くだけなら外しません）。

## 見送り

次の target は、その回は触りません。

- P が1つも無い（cargo の registry・git キャッシュなど、署名入りの `CACHEDIR.TAG` だけを持つ）。走査も R4 もせず、ログに「P なし」と残します。
- ごみ箱が有効、または判定できない。
- cargo が使用中（P ごとの `.cargo-lock`・`.cargo-build-lock`・`.cargo-artifact-lock` を排他ロックで取れない）。ロックは target ごとに全 P 分を取り、走査し直し・計画・削除が終わるまで持ちます。ロックのファイルは作りません。
- 走査で読めない項目がある（先頭10件をログへ）。
- 直近2時間以内に更新されたファイルがある。

Prunlet の1回の呼び出しは10分（`prunlet_timeout_secs`）で打ち切り、結果不明（`ambiguous`）として数え、再送しません。1件でも `ambiguous` が出たら、ロックを解放して実行全体を止めます（残りの target は処理せず、終了コード 3）。打ち切ったときは、削除の途中で build の一部だけが残っていることがあります。cargo は次のビルドで足りない分を作り直すだけなので、手当ては要りません。

## 記録と終了コード

- ログ: `<home>\logs\scopula-YYYY-MM-DD.log` に追記します。30日より古いログは、実行の最後に消します（`--dry-run` では消しません）。
- 終了コード: `0` 正常 / `1` 掃除後も警告の上限（既定 100GiB）を超える target がある / `2` 実行できなかった / `3` Prunlet の結果が不明で中断した。
- Everything の出力（`es-result.json`）と Prunlet の一時ファイルは `%TEMP%\scopula\<実行ID>` に作り、実行の最後に Prunlet で片付けます（`--dry-run` でもこの一時フォルダーだけは片付けます。中断したときは片付けずに残し、次の正常な実行（dry-run 以外）で、24時間より古い `%TEMP%\scopula\<実行ID>` を Prunlet で片付けます）。
- `warn_gib` が `cap_gib` より小さいと、起動時にログと標準エラーへ警告します。
- 表示するサイズは、ファイルの長さの合計です。ハードリンクされたファイルは複数の場所で数えるため、実際に空く量より多く見えることがあります。

## 設定（任意）

`<home>\config.toml` で既定値を上書きできます。書かなかった項目は既定値のままで、未知の項目はエラーです。

```toml
recent_hours = 2        # 直近この時間以内に更新があれば見送る
legacy_days = 7         # R1
incremental_days = 7    # R2
unit_days = 7           # R3 のユニットの古さ
seed_days = 7           # R3 の起点（P の最終ビルドからの幅）
tmp_days = 3            # R4
stale_profile_days = 30 # R6
cap_gib = 80            # R5 の上限
warn_gib = 100          # 掃除後もこれを超えると終了コード 1（cap_gib 以上にする）
exclude = []            # 探索から外すフォルダー。例: ['D:\cargo-home\registry']
es_path = ''            # es.exe の場所（空なら PATH、なければ WinGet の既定位置）
es_timeout_ms = 60000
# prunlet_exe = 'C:\path\to\prunlet.exe'      # 既定は %LOCALAPPDATA%\PrunletApp\current\prunlet.exe
# prunlet_config = 'C:\path\to\config.json'   # 既定は %USERPROFILE%\.local\Prunlet\config.json
prunlet_batch = 1000    # Prunlet の1回の削除に渡す件数（上限 1024）
prunlet_timeout_secs = 600
log_keep_days = 30
```

`prunlet_exe`・`prunlet_config` は、既定の場所へ置いてあれば書く必要はありません。別の場所に置いたときだけ、コメントを外して絶対パスで書いてください。

## ライセンス

MIT（[LICENSE](LICENSE)）。

## 導入（Install-Scopula.ps1）

PowerShell 7 から、リポジトリの `scripts\Install-Scopula.ps1` を実行します（Rust は `rust-toolchain.toml` の版を使います）。

```powershell
.\scripts\Install-Scopula.ps1 -DryRun      # 登録せず、内容だけを表示（-WhatIf でも同じ）
.\scripts\Install-Scopula.ps1              # ビルド・複製・設定の作成・タスク登録
```

| 引数 | 既定 | 内容 |
| --- | --- | --- |
| `-CargoTargetDirectory` | `%TEMP%\scopula-build\target` | ビルド成果物の置き場（リポジトリの外） |
| `-InstallDirectory` | `%USERPROFILE%\.local\Scopula` | `scopula.exe`・`config.toml`・`logs` の置き場 |

- `cargo build --release --locked` の後、`scopula.exe` を `-InstallDirectory` へ複製します。
- `config.toml` が無ければ、既定値をコメントにした雛形を作ります。あれば上書きしません。
- 削除の処理は持ちません。

### タスク

次のタスクを登録します（同名のタスクがあれば置き換えます）。

- 名前: `Scopula`
- 実行: 毎日 04:00。起動できなかった回は `StartWhenAvailable` で次の機会に実行します。
- 権限: 本人のログオン時だけ、通常の権限で実行します（ごみ箱の設定が HKCU にあるため、管理者や SYSTEM では動かしません）。
- 実行時間の上限: 2時間
- コマンド: `<InstallDirectory>\scopula.exe sweep --home "<InstallDirectory>"`

### アンインストール

タスクの登録解除は `Unregister-ScheduledTask -TaskName Scopula -Confirm:$false` です。`-InstallDirectory` のフォルダーの削除は、Prunlet CLI で行います（Scopula 自身は何も削除しません）。

```powershell
prunlet --config <prunlet の config.json> delete --recursive -- "$env:USERPROFILE\.local\Scopula"
```

## CI

`.github/workflows/scopula-tests.yml` が、main への push・PR・手動実行で、GitHub 提供の `windows-latest` 上で `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked` を流します。Windows 専用のツール（レジストリ・ボリューム判定・ロック・Everything）なので、Linux・macOS では動かしません。ごみ箱が有効なランナーでは、削除に進まず見送る経路を確かめ、削除の経路は判定を差し替える単体テストで確かめます。

依存の更新は Dependabot（`.github/dependabot.yml`、github-actions と cargo を毎週月曜）が PR を作り、`dependabot-auto-merge.yml` が CI の成功後に、依存の更新だけの PR を自動でマージします。

## ビルド

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\scopula-build\target"   # リポジトリの外に置く
cargo build --release
cargo test
```
