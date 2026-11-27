# CI とビルド

`.github/workflows/scopula-tests.yml` が、main への push・PR・手動実行で、GitHub 提供の `windows-latest` 上で `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked` を流します。Windows 専用のツール（レジストリ・ボリューム判定・ロック・Everything）なので、Linux・macOS では動かしません。ごみ箱が有効なランナーでは、削除に進まず見送る経路を確かめ、削除の経路は判定を差し替える単体テストで確かめます。

依存の更新は Dependabot（`.github/dependabot.yml`、github-actions と cargo を毎週月曜）が PR を作り、`dependabot-auto-merge.yml` が CI の成功後に、依存の更新だけの PR を自動でマージします。

## ビルド

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\scopula-build\target"   # リポジトリの外に置く
cargo build --release
cargo test
```
