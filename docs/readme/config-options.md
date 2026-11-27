# 設定（config.toml）

`<home>\config.toml` で既定値を上書きできます。書かなかった項目は既定値のままで、未知の項目はエラーです。`warn_gib` が `cap_gib` より小さいと、起動時にログと標準エラーへ警告します。

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

es.exe は、`es_path` → PATH → `%LOCALAPPDATA%\Microsoft\WinGet\Packages\voidtools.Everything.Cli_*` の順に探します。
