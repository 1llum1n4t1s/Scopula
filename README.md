# Scopula

Rust の `target` フォルダーに溜まる古い成果物を、定期的に掃除する CLI です。Cargo は古い成果物を消さず、target の自動掃除もないため、toolchain の更新や Cargo.lock の変更のたびに世代が溜まります。Scopula は全ドライブの target を探し、使われていない古い世代だけを消します。削除は自分では行わず、Prunlet CLI に任せます。

Windows 専用です（レジストリ・ボリューム判定・Everything を使います）。

## 入手とインストール

配布用のバイナリはありません。リポジトリから、PowerShell 7 で導入スクリプトを実行します（ビルド・複製・設定の雛形・毎日 04:00 のタスク登録を行います）。

```powershell
.\scripts\Install-Scopula.ps1 -DryRun   # 登録せず内容だけを表示
.\scripts\Install-Scopula.ps1           # 導入
```

引数・タスクの内容・アンインストールは [導入・タスク](docs/readme/install-task.md) を参照してください。手でビルドするときは [CI とビルド](docs/readme/ci-and-build.md) を参照してください。

## 使い方

```powershell
scopula sweep --dry-run          # 何も消さず、削除予定と見送り理由だけを表示
scopula sweep                    # 実際に掃除
scopula sweep --root D:\work     # Everything を使わず、指定フォルダーの下を走査（複数指定可）
scopula sweep --dry-run --json   # 結果を JSON で出力
scopula sweep --home D:\x        # 設定・ログの置き場を変える（既定は %LOCALAPPDATA%\Scopula）
```

- target は Everything の CLI（`es.exe`）で `CACHEDIR.TAG` を探して見つけます。応答しないときは終了コード 2 です。0 件のときは警告します。
- target と認めるのは、`CACHEDIR.TAG` が cargo 製のフォルダーだけです。
- `.git`・`node_modules` の下、ごみ箱の中、リンク（ジャンクション等）は対象外です。ほかに外したい場所は設定の `exclude` に書きます。
- 窓を出さない作りのため、手で実行するとシェルが終了を待たず、出力を取りこぼすことがあります。`start /wait scopula sweep --dry-run` のようにするか、`scopula sweep --dry-run | more` のようにパイプへつないでください。

## 戻せるか（重要）

削除は、ごみ箱が無効なボリューム（`HKCU\...\Explorer\BitBucket\Volume\{GUID}` の `NukeOnDelete=1`、またはポリシー `NoRecycleFiles=1`）にある target に限ります。有効なボリューム、または判定できないボリュームの target は、その回は見送ります。**消したものは戻せません**（永久削除になります）。

## 詳細

- [何を消すか（R1〜R6、使用中ユニットの判定）](docs/readme/rules-and-r3.md)
- [見送りと Prunlet の打ち切り](docs/readme/skip-conditions.md)
- [設定（config.toml）](docs/readme/config-options.md)
- [記録・終了コード・一時ファイル](docs/readme/logs-and-exit-codes.md)
- [導入・タスク・アンインストール](docs/readme/install-task.md)
- [CI とビルド](docs/readme/ci-and-build.md)
- 設計は [DESIGN.md](DESIGN.md)、変更履歴は [CHANGELOG.md](CHANGELOG.md)、開発者向けの規則は [AGENTS.md](AGENTS.md)

終了コードは `0` 正常 / `1` 掃除後も警告上限を超える target がある / `2` 実行できなかった / `3` Prunlet の結果が不明で中断、です。

## ライセンス

MIT（[LICENSE](LICENSE)）。
