# Changelog

このプロジェクトの利用者に見える変更を記録します。
形式は Keep a Changelog（https://keepachangelog.com/ja/1.1.0/）に、版番号は Semantic Versioning（https://semver.org/lang/ja/）に従います。

## [Unreleased]

### Added

- Rust の `target` フォルダーの古い成果物を掃除する CLI `scopula sweep` を追加。Everything で全ドライブの target を探し、使われていない古い世代だけを削除対象にする（`--dry-run`・`--root`・`--json`・`--home` に対応）。
- 規則 R1〜R6（旧配置・incremental・使用中でないユニット・tmp・容量上限・古いプロファイル）による選別を追加。
- 削除は Prunlet CLI に任せ、ごみ箱が無効と確認できたボリュームでだけ行う。
- タスクスケジューラーへ毎日登録する導入スクリプト `scripts\Install-Scopula.ps1` を追加。

### Fixed

- ビルド失敗の直後などに残る空（0 バイト）の指紋ファイルを壊れ扱いにして、その target の掃除を止めていた問題を修正。
