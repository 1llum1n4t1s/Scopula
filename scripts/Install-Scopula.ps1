<#
.SYNOPSIS
Scopula を release ビルドして導入し、毎日 04:00 に掃除するタスクを登録する。

.DESCRIPTION
1. cargo build --release --locked（CARGO_TARGET_DIR は -CargoTargetDirectory、リポジトリの外）
2. scopula.exe を -InstallDirectory へ複製（設定・ログの置き場も同じフォルダー）
3. config.toml が無ければ作る（あれば上書きしない）
4. タスク「Scopula」を登録（既存があれば置き換える）

登録は本人のログオン時だけ、通常の権限で行う（ごみ箱の設定が HKCU にあるため）。
-DryRun または -WhatIf では、ビルド・複製・登録をせず、内容だけを表示する。
削除の処理は持たない（アンインストールは README の手順）。
HKCU を書き換えないが、Claude デスクトップアプリの内側ではなく通常の PowerShell から実行すること。

.PARAMETER CargoTargetDirectory
ビルド成果物の置き場。既定は %TEMP%\scopula-build\target。

.PARAMETER InstallDirectory
exe・設定・ログの置き場。既定は %USERPROFILE%\.local\Scopula。

.PARAMETER DryRun
登録せず、内容だけを表示する。
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$CargoTargetDirectory = (Join-Path $env:TEMP 'scopula-build\target'),
    [string]$InstallDirectory = (Join-Path $env:USERPROFILE '.local\Scopula'),
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$preview = $DryRun.IsPresent -or $WhatIfPreference

$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoTarget = [System.IO.Path]::GetFullPath($CargoTargetDirectory)
$installDir = [System.IO.Path]::GetFullPath($InstallDirectory)
$builtExe = Join-Path $cargoTarget 'release\scopula.exe'
$installedExe = Join-Path $installDir 'scopula.exe'
$configPath = Join-Path $installDir 'config.toml'
$taskName = 'Scopula'
$taskArgument = 'sweep --home "{0}"' -f $installDir
$user = '{0}\{1}' -f $env:USERDOMAIN, $env:USERNAME

# 既定値のまま動くので、すべてコメントにしておく（変えたい項目だけ外す）。
$configTemplate = @'
# Scopula の設定。書かなかった項目は既定値のままです（未知の項目はエラー）。
# recent_hours = 2        # 直近この時間以内に更新があれば見送る
# legacy_days = 7         # R1
# incremental_days = 7    # R2
# unit_days = 7           # R3 のユニットの古さ
# seed_days = 7           # R3 の起点（P の最終ビルドからの幅）
# tmp_days = 3            # R4
# stale_profile_days = 30 # R6
# cap_gib = 80            # R5 の上限
# warn_gib = 100          # 掃除後もこれを超えると終了コード 1（cap_gib 以上にする）
# exclude = ['D:\cargo-home\registry']   # 探索から外すフォルダー
# es_path = ''            # es.exe の場所（空なら PATH、なければ WinGet の既定位置）
# es_timeout_ms = 60000
# prunlet_exe = 'C:\path\to\prunlet.exe'      # 既定は %LOCALAPPDATA%\PrunletApp\current\prunlet.exe
# prunlet_config = 'C:\path\to\config.json'   # 既定は %USERPROFILE%\.local\Prunlet\config.json
# prunlet_batch = 1000
# prunlet_timeout_secs = 600
# log_keep_days = 30
'@

if ($preview) {
    Write-Host '[DryRun] 何も変更しません。実行する内容:'
}

# 1. ビルド
Write-Host ('1. release ビルド: CARGO_TARGET_DIR={0}' -f $cargoTarget)
Write-Host ('   cargo build --release --locked  (作業フォルダー: {0})' -f $repoRoot)
if (-not $preview) {
    $env:CARGO_TARGET_DIR = $cargoTarget
    Push-Location $repoRoot
    try {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw "cargo build が失敗しました (終了コード $LASTEXITCODE)" }
    }
    finally {
        Pop-Location
    }
    if (-not (Test-Path -LiteralPath $builtExe)) { throw "ビルド成果物がありません: $builtExe" }
}

# 2. exe の複製
Write-Host ('2. 複製: {0} -> {1}' -f $builtExe, $installedExe)
if (-not $preview) {
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -LiteralPath $builtExe -Destination $installedExe -Force
}

# 3. 設定ファイル（あれば触らない）
if (Test-Path -LiteralPath $configPath) {
    Write-Host ('3. 設定ファイル: 既にあるため触りません ({0})' -f $configPath)
}
else {
    Write-Host ('3. 設定ファイル: 新規作成します ({0})' -f $configPath)
    if (-not $preview) {
        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        [System.IO.File]::WriteAllText($configPath, $configTemplate, (New-Object System.Text.UTF8Encoding($false)))
    }
}

# 4. タスクの登録
$existing = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
Write-Host ('4. タスク登録: 名前={0} {1}' -f $taskName, $(if ($existing) { '(既存を置き換える)' } else { '(新規)' }))
Write-Host ('   実行: {0} {1}' -f $installedExe, $taskArgument)
Write-Host '   契機: 毎日 04:00（StartWhenAvailable: 逃した回は次の機会に実行）'
Write-Host ('   権限: {0} のログオン時だけ、通常の権限 (Interactive / Limited)' -f $user)
Write-Host '   実行時間の上限: 2時間'
if (-not $preview) {
    $action = New-ScheduledTaskAction -Execute $installedExe -Argument $taskArgument -WorkingDirectory $installDir
    $trigger = New-ScheduledTaskTrigger -Daily -At '04:00'
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Hours 2) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description 'Rust の target の古い成果物を掃除する (Scopula)' -Force | Out-Null
    Write-Host ('登録しました: {0}' -f $taskName)
}
