# 導入・タスク・アンインストール

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

## タスク

次のタスクを登録します（同名のタスクがあれば置き換えます）。

- 名前: `Scopula`
- 実行: 毎日 04:00。起動できなかった回は `StartWhenAvailable` で次の機会に実行します。
- 権限: 本人のログオン時だけ、通常の権限で実行します（ごみ箱の設定が HKCU にあるため、管理者や SYSTEM では動かしません）。
- 実行時間の上限: 2時間
- コマンド: `<InstallDirectory>\scopula.exe sweep --home "<InstallDirectory>"`

## アンインストール

タスクの登録解除は `Unregister-ScheduledTask -TaskName Scopula -Confirm:$false` です。`-InstallDirectory` のフォルダーの削除は、Prunlet CLI で行います（Scopula 自身は何も削除しません）。

```powershell
prunlet --config <prunlet の config.json> delete --recursive -- "$env:USERPROFILE\.local\Scopula"
```
