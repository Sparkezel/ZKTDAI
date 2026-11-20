# ZKTDAI headless game runner (Windows-native, no WSL needed). Linux/WSL2: use run.sh.
#
# Downloads the Spring engine (win64) + Zero-K game + a map into a work dir,
# installs the ZKTDAI build, writes script.txt, and runs N AI-vs-AI games with
# spring-headless.exe. Results land in <WorkDir>\out\results.md and can be
# posted to a GitHub issue.
#
# Example (local v0.2 build vs CircuitAI Normal on Altored Divide, 3 games):
#   powershell -File tools\headless\run.ps1 -Games 3 -TraceEvents -PostIssue 2
#
# Use a CI artifact instead of a local build (SkirmishAI.dll):
#   -AiZip "https://github.com/Sparkezel/ZKTDAI/actions/artifacts/<id>/zip"
# or download one with:  gh run download <run-id> -n ZKTDAI-0.2.0 -D <dir>
# (build the dll locally with:  tools\package.sh --cross)
#
# Requires: curl.exe (ships with Windows 10+), PowerShell 5.1+, and for
# -PostIssue the gh CLI. See README.md for all knobs.
param(
    [string]$EngineVer = "2026.07.04",
    [string]$MapName = "Altored Divide Bar Remake 1.6",
    [string]$MapFile = "",                  # e.g. altored_divide_bar_remake_1.6.sd7 (auto-derived if empty)
    [string]$GameType = "",                 # auto-detected from the game's modinfo.lua if empty
    [string]$GameTagUrl = "https://repos.springrts.com/zk/tags/zk:stable",
    [string]$Opp = "CircuitAI:Normal",      # opponent AI as short:version
    [int]$Games = 1,
    [string]$WorkDir = "$env:USERPROFILE\.zktdai-headless",
    [string]$AiZip = "",                    # CI artifact zip (AI/Skirmish/ZKTDAI/<ver>/... layout) or URL
    [string]$AiDir = "",                    # or a loose dir with AIInfo.lua, AIOptions.lua, SkirmishAI.dll
    [string]$Version = "",                  # AI version; default read from crates/zktdai/Cargo.toml
    [int]$PostIssue = 0,                    # gh issue number to comment the results on (0 = no)
    [switch]$TraceEvents,                   # run games with ZKTDAI_TRACE_EVENTS=1
    [int]$StartPos = 2,                     # 0 random, 1 fixed, 2 chosen (AIs send start pos)
    [int]$MaxUnits = 2000,
    [int]$TimeoutSec = 1800,
    [string]$ExtraArgs = ""
)
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path "$PSScriptRoot\..\..").Path
$Out = Join-Path $WorkDir "out"
New-Item -ItemType Directory -Force -Path $WorkDir, "$WorkDir\downloads", "$WorkDir\games", "$WorkDir\maps", $Out | Out-Null

if (-not $Version) {
    $Version = ((Select-String -Path "$RepoRoot\crates\zktdai\Cargo.toml" -Pattern '^version\s*=\s*"(.*)"' | Select-Object -First 1).Line -replace '.*"(.*)".*', '$1')
}
if (-not $MapFile) {
    $MapFile = ($MapName.ToLower() -replace ' ', '_') + ".sd7"
}
$OppShort, $OppVer = $Opp -split ':', 2

function Get-File($Url, $Dest) {
    if (Test-Path $Dest) { return }
    Write-Host "   GET $Url"
    curl.exe -fsSL -o $Dest $Url
}

Write-Host "== engine $EngineVer (win64)"
$EngineZip = Join-Path $WorkDir "downloads\engine-$EngineVer-win64.zip"
Get-File "https://zero-k.info/engine/win64/$EngineVer.zip" $EngineZip
$EngineBin = Get-ChildItem -Recurse -Path $WorkDir -Filter "spring-headless.exe" -File |
    Select-Object -First 1 -ExpandProperty FullName
if (-not $EngineBin) {
    Expand-Archive -Force -Path $EngineZip -DestinationPath $WorkDir
    $EngineBin = Get-ChildItem -Recurse -Path $WorkDir -Filter "spring-headless.exe" -File |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $EngineBin) { throw "spring-headless.exe not found after extracting $EngineZip" }
$EngineDir = Split-Path $EngineBin
Write-Host "   engine binary: $EngineBin"

Write-Host "== game (rapid zk:stable)"
$Tag = curl.exe -fsSL $GameTagUrl
$Pkg = ([regex]::Matches($Tag, 'packages/[A-Za-z0-9._-]+\.sdp') | Select-Object -Last 1).Value
if (-not $Pkg) { throw "no .sdp found in $GameTagUrl" }
$GameSdp = Join-Path $WorkDir "games\zk.sdp"
if (-not (Test-Path $GameSdp)) {
    $ok = $false
    foreach ($base in @("https://repos.springrts.com/zk/stable", "https://repos.springrts.com/zk")) {
        try { Get-File "$base/$Pkg" $GameSdp; $ok = $true; break } catch { }
    }
    if (-not $ok) { throw "failed to download $Pkg" }
}
if (-not $GameType) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($GameSdp)
    try {
        $entry = $zip.Entries | Where-Object { $_.FullName -eq "modinfo.lua" } | Select-Object -First 1
        if ($entry) {
            $reader = New-Object System.IO.StreamReader($entry.Open())
            $modinfo = $reader.ReadToEnd(); $reader.Close()
            $m = [regex]::Match($modinfo, '(?m)^\s*name\s*=\s*"(.*)"')
            if ($m.Success) { $GameType = $m.Groups[1].Value }
        }
    } finally { $zip.Dispose() }
    if (-not $GameType) { $GameType = "Zero-K" }
}
Write-Host "   game: $GameType ($Pkg)"

Write-Host "== map: $MapName ($MapFile)"
$MapSd7 = Join-Path $WorkDir "maps\$MapFile"
Get-File "https://zero-k.info/content/maps/$MapFile" $MapSd7

Write-Host "== AI: ZKTDAI $Version"
$AiDest = Join-Path $WorkDir "AI\Skirmish\ZKTDAI\$Version"
New-Item -ItemType Directory -Force -Path $AiDest | Out-Null
if ($AiZip) {
    $zipLocal = $AiZip
    if ($AiZip -match '^https?://') {
        $zipLocal = Join-Path $WorkDir "downloads\ai.zip"
        Get-File $AiZip $zipLocal
    }
    Expand-Archive -Force -Path $zipLocal -DestinationPath $WorkDir
    if (-not (Test-Path $AiDest)) { throw "AI zip has no AI/Skirmish/ZKTDAI/$Version/" }
} elseif ($AiDir) {
    (Get-Content "$AiDir\AIInfo.lua") -replace '@VERSION@', $Version | Set-Content "$AiDest\AIInfo.lua"
    Copy-Item "$AiDir\AIOptions.lua" "$AiDest\AIOptions.lua"
    Copy-Item "$AiDir\SkirmishAI.dll" "$AiDest\SkirmishAI.dll"
} else {
    $lib = @(
        "$RepoRoot\target\release\SkirmishAI.dll",
        "$RepoRoot\target\x86_64-pc-windows-gnu\release\SkirmishAI.dll"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $lib) { throw "no local SkirmishAI.dll found — run tools\package.sh --cross, or pass -AiZip/-AiDir" }
    (Get-Content "$RepoRoot\crates\zktdai\data\AIInfo.lua") -replace '@VERSION@', $Version | Set-Content "$AiDest\AIInfo.lua"
    Copy-Item "$RepoRoot\crates\zktdai\data\AIOptions.lua" "$AiDest\AIOptions.lua"
    Copy-Item $lib "$AiDest\SkirmishAI.dll"
}
Write-Host "   installed at $AiDest"

Write-Host "== spring settings + script.txt"
$sd = (Resolve-Path $WorkDir).Path -replace '\\', '/'
"SpringData = $sd" | Set-Content "$EngineDir\springsettings.cfg"
$tpl = Get-Content "$RepoRoot\tools\headless\script.txt.template" -Raw
$tpl = $tpl -replace '@GAMETYPE@', $GameType -replace '@MAP@', $MapName -replace '@STARTPOS@', "$StartPos" `
    -replace '@MAXUNITS@', "$MaxUnits" -replace '@AI_NAME@', "ZKTDAI $Version (1)" -replace '@AI_SHORT@', 'ZKTDAI' `
    -replace '@AI_VERSION@', $Version -replace '@OPP_NAME@', "$OppShort $OppVer (1)" -replace '@OPP_SHORT@', $OppShort `
    -replace '@OPP_VERSION@', $OppVer
$tpl | Set-Content "$WorkDir\script.txt"

Write-Host "== running $Games game(s)"
$results = Join-Path $Out "results.md"
"| game | map | opponent | result | frames | exit |" | Set-Content $results
"|---|---|---|---|---|---|" | Add-Content $results
$envName = "ZKTDAI_TRACE_EVENTS"
for ($i = 1; $i -le $Games; $i++) {
    Remove-Item "$WorkDir\infolog.txt" -ErrorAction SilentlyContinue
    if ($TraceEvents) { $env:ZKTDAI_TRACE_EVENTS = "1" } else { Remove-Item "Env:\$envName" -ErrorAction SilentlyContinue }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $EngineBin
    $psi.Arguments = $ExtraArgs
    $psi.WorkingDirectory = $WorkDir
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $p = [System.Diagnostics.Process]::Start($psi)
    $p.WaitForExit($TimeoutSec * 1000) | Out-Null
    $code = if ($p.HasExited) { $p.ExitCode } else { $p.Kill(); -1 }
    $p.StandardOutput.ReadToEnd() | Out-File "$Out\console-$i.log"
    $p.StandardError.ReadToEnd() | Out-File "$Out\console-$i.err.log" -Append
    if (Test-Path "$WorkDir\infolog.txt") { Copy-Item "$WorkDir\infolog.txt" "$Out\infolog-game$i.txt" }
    $log = "$Out\infolog-game$i.txt"
    $over = "no game-over line"
    $res = "?"
    $frames = "?"
    if (Test-Path $log) {
        $line = Select-String -Path $log -Pattern '(?i)game over' | Select-Object -First 1
        if ($line) { $over = $line.Line.Trim() }
        # ZKTDAI is allyteam 0: if allyteam 1 met the game-over conditions, we won
        if ($over -match 'Allyteam 1') { $res = "WIN" } elseif ($over -match 'Allyteam 0') { $res = "LOSS" }
        $f = [regex]::Matches((Get-Content $log -Raw), 'f=[-0-9]+') | Select-Object -Last 1
        if ($f) { $frames = $f.Value }
    }
    Write-Host "game ${i}: result=$res ($over) $frames exit=$code"
    "| $i | $MapName | $Opp | $res | $frames | $code |" | Add-Content $results
}
Remove-Item "Env:\$envName" -ErrorAction SilentlyContinue

Write-Host "== results"
Get-Content $results
Write-Host "infologs: $Out\infolog-game*.txt"

if ($PostIssue -gt 0 -and (Get-Command gh -ErrorAction SilentlyContinue)) {
    $body = @"
## Headless run — ZKTDAI $Version vs $Opp on $MapName (engine $EngineVer)

$(Get-Content $results -Raw)

infologs: see run output (out\infolog-game*.txt)
"@
    $body | Set-Content "$Out\issue-comment.md"
    gh issue comment $PostIssue --repo Sparkezel/ZKTDAI --body-file "$Out\issue-comment.md"
    Write-Host "posted results to issue #$PostIssue"
}
