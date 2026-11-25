<#
.SYNOPSIS
  One run of the MCP dogfood loop: rebuild, start a serve from the fresh build, seed a blank
  document, run a headless tester against that serve, and collect what it left. -Check only
  lists the hosts.

.DESCRIPTION
  The tester runs from the run folder with no repository, no user or project settings, no
  skills and only the MCP tools (and ToolSearch, so they reach it as they reach a real agent).
  Its proxy is the fresh build's; the serve it finds is the one this script started, so a
  stale serve cannot answer it. A window or a foreign serve showing up during the run makes the
  run invalid.

  -Prototypes reads briefs-prototype.md, whose entries are a surface and a prototype. With
  -Setup the run is the setup pass instead: it builds the entry's surface from setup.md and
  leaves its manifest in surface.json. Without -Setup, -Seed starts the run from that surface
  (a copy of the setup run's page.mastik) instead of an empty document.

  Exit codes: 0 a report was collected; 1 the script failed; 3 a window is running;
  4 a serve from another build is running; 5 a window or foreign host appeared during the run;
  6 the tester ended without a report (crash, budget, no JSON block).
  The last line printed is the run folder.
#>
param(
    [int]$Run = 1,
    [int]$BriefIndex = -1,
    [string]$BriefFile,
    [switch]$Prototypes,
    [switch]$Setup,
    [string]$Seed,
    [switch]$AllTools,
    [string]$Photos = $env:MASTIK_DOGFOOD_PHOTOS,
    [string]$Model = 'opus',
    [double]$BudgetUsd = 20,
    [string]$Out,
    [switch]$Check,
    [switch]$NoBuild
)

# Native tools write progress to stderr; their exit codes decide, not the stream.
$ErrorActionPreference = 'Continue'
$utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8

$skill = Split-Path -Parent $PSScriptRoot
$repo = (Resolve-Path (Join-Path $skill '..\..\..')).Path
$exe = Join-Path $repo 'target\release\mastik.exe'
$mcpCopy = Join-Path $env:LOCALAPPDATA 'Mastik-mcp'

function Stop-Run([int]$code, [string]$why) {
    [Console]::Error.WriteLine("run-tester: $why")
    exit $code
}

# Runs mastik.exe and answers its exit code. It is a GUI-subsystem exe, for which PowerShell
# neither waits nor sets $LASTEXITCODE when called with `&`.
function Invoke-Mastik([string[]]$arguments) {
    $quoted = $arguments | ForEach-Object { '"' + $_ + '"' }
    $p = Start-Process -FilePath $exe -ArgumentList $quoted -Wait -PassThru -NoNewWindow
    $p.ExitCode
}

function Get-Hosts {
    $dir = Join-Path $env:LOCALAPPDATA 'MastikData\mcp'
    if (-not (Test-Path $dir)) { return @() }
    foreach ($f in Get-ChildItem $dir -Filter '*.json') {
        try { $found = Get-Content $f.FullName -Raw | ConvertFrom-Json } catch { continue }
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$($found.pid)" -ErrorAction SilentlyContinue
        if ($p) { [pscustomobject]@{ Pid = $found.pid; Host = $found.host; Exe = $p.ExecutablePath } }
    }
}

function Get-Serves {
    Get-CimInstance Win32_Process -Filter "Name='mastik.exe'" |
        Where-Object { $_.CommandLine -match '\sserve\s*$' }
}

# Stops the serves this loop owns: the fresh build's and the MCP copy's, which would otherwise
# hold the one serve lock with a stale build. A serve from any other install is someone's work.
function Stop-Serve {
    $serves = @(Get-Serves)
    $foreign = @($serves | Where-Object {
            $_.ExecutablePath -ne $exe -and -not $_.ExecutablePath.StartsWith($mcpCopy, 'OrdinalIgnoreCase')
        })
    if ($foreign.Count -gt 0) {
        Stop-Run 4 ("a serve from another build is running: " + (($foreign | ForEach-Object { "$($_.ProcessId) $($_.ExecutablePath)" }) -join '; '))
    }
    foreach ($p in $serves) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
    # Its exe stays locked until the process has gone.
    foreach ($p in $serves) { Wait-Process -Id $p.ProcessId -Timeout 10 -ErrorAction SilentlyContinue }
}

function Find-Claude {
    $onPath = Get-Command claude -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    # The desktop app's bundled CLI, in a folder named by its version.
    $bundled = Get-ChildItem (Join-Path $env:APPDATA 'Claude\claude-code\*\*\claude.exe') -ErrorAction SilentlyContinue |
        Sort-Object {
            $v = $null
            if ([version]::TryParse(($_.Directory.Parent.Name -replace '[^0-9.].*$', ''), [ref]$v)) { $v } else { [version]'0.0' }
        } |
        Select-Object -Last 1
    if (-not $bundled) { Stop-Run 1 'no claude CLI on PATH or in the desktop app' }
    $bundled.FullName
}

# The numbered entries under a `## <heading>` of briefs.md, each joined onto one line.
function Get-Entries([string]$text, [string]$heading) {
    $section = ($text -split "(?m)^## $heading\s*$", 2)
    if ($section.Count -lt 2) { Stop-Run 1 "briefs.md has no '## $heading' section" }
    $body = ($section[1] -split '(?m)^## ', 2)[0]
    $entries = @(($body -split '(?m)^\d+\.\s+' | Select-Object -Skip 1) | ForEach-Object { ($_ -replace '\s+', ' ').Trim() })
    if ($entries.Count -eq 0) { Stop-Run 1 "briefs.md's '## $heading' lists nothing" }
    $entries
}

$hosts = @(Get-Hosts)
if ($Check) {
    if ($hosts.Count -eq 0) { 'no Mastik host is running' } else { $hosts | Format-Table -AutoSize | Out-String -Width 200 }
    exit 0
}
if ($hosts | Where-Object Host -eq 'window') {
    Stop-Run 3 'a Mastik window is running; the tester would edit it. Ask the person to close it.'
}

if ($Setup -and -not $Prototypes) { Stop-Run 1 '-Setup builds a prototype surface; give -Prototypes too' }
if ($Seed -and -not (Test-Path $Seed)) { Stop-Run 1 "no seed document at $Seed" }
$briefsName = if ($Prototypes) { 'briefs-prototype.md' } else { 'briefs.md' }
$briefs = [IO.File]::ReadAllText((Join-Path $skill $briefsName), $utf8)
$pool = @(Get-Entries $briefs 'Pool')
if ($BriefFile) {
    $brief = ([IO.File]::ReadAllText($BriefFile, $utf8) -replace '\s+', ' ').Trim()
} elseif ($BriefIndex -ge 0) {
    $brief = $pool[$BriefIndex % $pool.Count]
} else {
    Stop-Run 1 'give -BriefIndex <n> or -BriefFile <path>'
}
# A prototype entry is its surface, then its prototype after `Prototype:`.
$surface = ''
if ($Prototypes -and -not $BriefFile) {
    $halves = $brief -split '\s*Prototype:\s*', 2
    if ($halves.Count -lt 2) { Stop-Run 1 "brief $BriefIndex has no 'Prototype:' half" }
    $surface = ($halves[0] -replace '^Surface:\s*', '').Trim()
    $brief = "The document already holds this design, made in an earlier session: $surface " +
        "Don't redesign it: read it, then make it play. $($halves[1].Trim())"
}
$passes = @(Get-Entries $briefs 'The passes')
$passText = (0..($passes.Count - 1) | ForEach-Object { "$($_ + 1). $($passes[$_])" }) -join "`n"

Stop-Serve
if (-not $NoBuild) {
    Push-Location $repo
    cargo build --release -p mastik-app
    $built = $LASTEXITCODE
    Pop-Location
    if ($built -ne 0) { Stop-Run 1 "cargo build failed ($built)" }
}

if (-not $Out) {
    $Out = Join-Path $env:TEMP ('mastik-dogfood\' + (Get-Date -Format 'yyyyMMdd-HHmmss') + "-run$Run")
}
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$doc = Join-Path $Out 'page.mastik'

if ($Seed -and -not $Setup) {
    Copy-Item $Seed $doc -Force
} else {
    $seeded = Invoke-Mastik @('run', (Join-Path $skill 'seed.luau'), '-o', $doc)
    if ($seeded -ne 0) { Stop-Run 1 "seeding the document failed ($seeded)" }
}

$photoNames = @()
if ($Photos -and (Test-Path $Photos)) {
    $into = New-Item -ItemType Directory -Force (Join-Path $Out 'photos')
    Get-ChildItem $Photos -File | Where-Object { $_.Extension -in '.jpg', '.jpeg', '.png', '.webp' } |
        Copy-Item -Destination $into
    $photoNames = @(Get-ChildItem $into -File | ForEach-Object { $_.FullName })
}
if ($photoNames.Count -gt 0) {
    $photoText = "Photos you may use, as files: " + ($photoNames -join ', ') + '.'
} else {
    Write-Warning 'no photos for this run (-Photos or MASTIK_DOGFOOD_PHOTOS); pass 2 will have none to place'
    $photoText = 'There are no photo files for this run; get imagery however the tools allow, or say what stopped you.'
}

if ($Setup) {
    $prompt = [IO.File]::ReadAllText((Join-Path $skill 'setup.md'), $utf8)
    $prompt = $prompt.Replace('{{SURFACE}}', $surface).Replace('{{PHOTOS}}', $photoText)
} else {
    $prompt = [IO.File]::ReadAllText((Join-Path $skill 'tester.md'), $utf8)
    $prompt = $prompt.Replace('{{BRIEF}}', $brief).Replace('{{PASSES}}', $passText).Replace('{{PHOTOS}}', $photoText)
}
[IO.File]::WriteAllText((Join-Path $Out 'prompt.md'), $prompt, $utf8)

$proxyArgs = @('mcp-proxy')
if ($AllTools) { $proxyArgs += @('--tools', 'all') }
$config = @{ mcpServers = @{ mastik = @{ command = $exe; args = $proxyArgs } } } | ConvertTo-Json -Depth 5
$configPath = Join-Path $Out 'mcp.json'
[IO.File]::WriteAllText($configPath, $config, $utf8)

# The serve the tester's proxy will find: started here from the fresh build, so a stale
# one cannot take the lock first.
$serve = Start-Process $exe -ArgumentList 'serve' -WindowStyle Hidden -PassThru
$deadline = (Get-Date).AddSeconds(15)
while (-not (Get-Hosts | Where-Object { $_.Pid -eq $serve.Id })) {
    if ((Get-Date) -gt $deadline -or $serve.HasExited) { Stop-Run 1 'the fresh serve did not start listening' }
    Start-Sleep -Milliseconds 200
}

$claude = Find-Claude
# Empty values are spelled `--flag=`: Windows PowerShell drops an empty argument.
$claudeArgs = @(
    '-p',
    '--model', $Model,
    '--mcp-config', $configPath,
    '--strict-mcp-config',
    '--setting-sources=project',
    '--disable-slash-commands',
    '--no-chrome',
    '--tools=ToolSearch',
    '--allowedTools', 'mcp__mastik',
    '--permission-mode', 'dontAsk',
    '--output-format', 'json',
    '--max-budget-usd', "$BudgetUsd"
)

Push-Location $Out
$answer = $prompt | & $claude @claudeArgs
$code = $LASTEXITCODE
Pop-Location

$intruders = @(Get-Hosts | Where-Object { $_.Pid -ne $serve.Id })
Stop-Serve
$answerText = ($answer -join "`n")
[IO.File]::WriteAllText((Join-Path $Out 'answer.json'), $answerText, $utf8)
if ($intruders.Count -gt 0) {
    $list = ($intruders | ForEach-Object { "$($_.Host) $($_.Pid) $($_.Exe)" }) -join '; '
    [IO.File]::WriteAllText((Join-Path $Out 'invalid.txt'), "other hosts ran during the run: $list", $utf8)
}

# The setup pass ends on its manifest, the tester on its findings.
$collected = if ($Setup) { 'surface.json' } else { 'findings.json' }
$parsed = $null
try { $parsed = $answerText | ConvertFrom-Json } catch { }
if ($parsed -and $parsed.result) {
    [IO.File]::WriteAllText((Join-Path $Out 'report.md'), [string]$parsed.result, $utf8)
    $block = [regex]::Matches([string]$parsed.result, '```json\s*([\s\S]*?)```')
    if ($block.Count -gt 0) {
        [IO.File]::WriteAllText((Join-Path $Out $collected), $block[$block.Count - 1].Groups[1].Value.Trim(), $utf8)
    }
}

# The tester's session, and from it one line per tool call: what was asked, whether it failed,
# and the start of the answer.
if ($parsed -and $parsed.session_id) {
    $claudeDir = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $env:USERPROFILE '.claude' }
    $log = Join-Path $claudeDir ("projects\" + ($Out -replace '[^A-Za-z0-9]', '-') + "\$($parsed.session_id).jsonl")
    if (-not (Test-Path $log)) {
        $log = (Get-ChildItem (Join-Path $claudeDir 'projects') -Recurse -Filter "$($parsed.session_id).jsonl" -ErrorAction SilentlyContinue |
                Select-Object -First 1).FullName
    }
    if ($log -and (Test-Path $log)) {
        Copy-Item $log (Join-Path $Out 'transcript.jsonl')
        $uses = [ordered]@{}
        $results = @{}
        foreach ($line in [IO.File]::ReadAllLines($log, $utf8)) {
            try { $entry = $line | ConvertFrom-Json } catch { continue }
            foreach ($part in @($entry.message.content)) {
                if ($part.type -eq 'tool_use') { $uses[$part.id] = $part }
                elseif ($part.type -eq 'tool_result') { $results[$part.tool_use_id] = $part }
            }
        }
        $n = 0
        $calls = foreach ($id in $uses.Keys) {
            $n++
            $use = $uses[$id]
            $res = $results[$id]
            $text = if ($res) { (@($res.content) | ForEach-Object { if ($_ -is [string]) { $_ } else { $_.text } }) -join ' ' } else { '' }
            if ($text.Length -gt 400) { $text = $text.Substring(0, 400) + '...' }
            @{ n = $n; tool = $use.name; args = $use.input; error = [bool]($res -and $res.is_error); answer = $text } |
                ConvertTo-Json -Depth 20 -Compress
        }
        [IO.File]::WriteAllText((Join-Path $Out 'calls.jsonl'), (@($calls) -join "`n"), $utf8)
    } else {
        Write-Warning "no transcript for session $($parsed.session_id)"
    }
}

if ((Invoke-Mastik @('export', $doc, '--png', '-o', (Join-Path $Out 'page.png'))) -ne 0) {
    Write-Warning 'rendering page.png failed'
}

$Out
if ($intruders.Count -gt 0) { Stop-Run 5 "invalid run: $(Get-Content (Join-Path $Out 'invalid.txt'))" }
if ($code -ne 0 -or -not $parsed -or $parsed.is_error -or -not (Test-Path (Join-Path $Out $collected))) {
    $why = if ($parsed) { "subtype $($parsed.subtype), is_error $($parsed.is_error)" } else { 'no JSON result' }
    Stop-Run 6 "the tester ended without a report (claude exit $code, $why)"
}
exit 0
