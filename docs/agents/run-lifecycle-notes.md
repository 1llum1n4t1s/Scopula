# 実行・終了コード・運用の設計メモ

- 終了コード（0・1・2・3）は[記録・終了コード・一時ファイル](../readme/logs-and-exit-codes.md)に集約している。変更したら README の末尾の一覧も直す。
- 中断（Prunlet の結果不明）のときは、Prunlet が固まっている可能性があるので一時フォルダーの片付けを呼ばず残す。残ったものは、次の正常な実行（dry-run以外）で、24時間より古いものだけ`stale_run_dirs`が選び、Prunletで消す。
- 登録（タスクスケジューラー）とインストールは別の担当が行う（`register.ps1`は置かない）。導入は`scripts/Install-Scopula.ps1`。
