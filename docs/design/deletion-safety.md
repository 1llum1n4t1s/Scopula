# 削除の安全境界

- ごみ箱判定（`trash.rs`）: ポリシー `NoRecycleFiles` を優先し、なければボリュームの `NukeOnDelete` を見る。無効（`Trash::Disabled`）だけが削除可。有効・判定不能は理由つきで見送る。無効なボリュームの削除は戻せない。
- ロック（`scan::acquire_locks`）: P ごとの `.cargo-lock`・`.cargo-build-lock`・`.cargo-artifact-lock` を `File::try_lock` で取る。ファイルは作らず、共有モードを0にしない（cargo を止めない）。取れなければ見送る。
- 直近更新: 直近2時間以内に更新があった target は触らない。
- Prunlet 呼び出し（`prune.rs`）: 候補を件数・バイト数で分割し、一時フォルダーの JSON で渡す。1回の呼び出しを `prunlet_timeout_secs` で打ち切り（プロセスを止めて終了を確かめる）、`ambiguous` として再送しない。出力が空・読めない場合や Prunlet 自身が返した `ambiguous` も同じで、その分割より後は実行せず、実行全体を止める（終了コード 3）。起動できない・要求が丸ごと拒否された場合は `failed`（何も削除していない）で、再送しないが止めない。
- 一時フォルダー: `%TEMP%\scopula\<実行ID>` の片付けも Prunlet で行う。中断で残ったものは、次の dry-run 以外の実行で、24時間より古いものだけ消す。
- テスト: 削除の経路は `trash_of` を注入した単体テストと、偽の Prunlet を使う `tests/cli.rs` で確かめる。実データへは `--dry-run` だけを向ける。
