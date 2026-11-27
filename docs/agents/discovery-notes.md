# 探索（target の見つけ方）の設計メモ

- `es.exe`の標準出力はANSIコードページで日本語が化けるため、`-export-json`（UTF-8）を`%TEMP%\scopula\<実行ID>\es-result.json`へ書き出して読む。`-dc`などの索引外の列は使わない（Everythingが固まる）。
- target判定は`CACHEDIR.TAG`の本文（cargo製）。P判定は直下の`.cargo-lock`の有無で、cargo製の`CACHEDIR.TAG`を持つtriple（`<triple>\debug`など）の子も見る。
- 探索結果は`fs::canonicalize`で実体のパスにそろえ（`\\?\`・`\\?\UNC\`は外す）、ジャンクション経由の重複を除く。別のtargetの内側にあるtargetは除く。
- 走査エラーが1件でもあるtargetは見送る（先頭10件を記録）。
- Pが1つも無いtarget（cargoのregistry・gitキャッシュなど、署名入りのCACHEDIR.TAGだけを持つもの）は、走査もR4もせず「P なし」で見送る。
