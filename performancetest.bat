@echo off
setlocal
cd /d "%~dp0"

for /f %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set RUN_ID=%%t
set REPORTS=%~dp0reports
set RESULTS=%REPORTS%\performance_%RUN_ID%
set SCRIPT=%~f0
set CARGO_PROFILE_RELEASE_DEBUG=true
set NETWORK=easy-db-migrator-profile
set POSTGRES=easy-db-migrator-profile-postgres
set MSSQL=easy-db-migrator-profile-mssql
set SA_PASSWORD=yourStrong(!)Password
set STEPS=8
set STEP_NUMBER=0

set STEP=checking docker
call :begin
docker info >nul 2>&1 || goto :nodocker
call :done

set STEP=building the profile image in release mode with debug information
call :begin
REM the lock file is not in git, and the image build copies it
if not exist Cargo.lock cargo generate-lockfile || goto :fail
docker build -f Dockerfile --target profile --build-arg CARGO_PROFILE_RELEASE_DEBUG -t easy-db-migrator:profile . || goto :fail
call :done

set STEP=starting PostgreSQL and SQL Server
call :begin
call :remove_the_databases
docker network create %NETWORK% >nul || goto :fail
docker run -d --name %POSTGRES% --network %NETWORK% -e POSTGRES_PASSWORD=postgres postgres:17-alpine >nul || goto :fail
docker run -d --name %MSSQL% --network %NETWORK% -e ACCEPT_EULA=Y -e MSSQL_PID=Developer -e "MSSQL_SA_PASSWORD=%SA_PASSWORD%" mcr.microsoft.com/mssql/server:2022-CU27-ubuntu-22.04 >nul || goto :fail
call :done

set STEP=profiling the migrator with hyperfine, cargo-flamegraph and dhat
call :begin
mkdir "%RESULTS%" || goto :fail
docker run --rm --privileged --network %NETWORK% -v "%RESULTS%:/results" -e "POSTGRES_URL=postgres://postgres:postgres@%POSTGRES%:5432" -e "MSSQL_URL=Server=tcp:%MSSQL%,1433;User Id=sa;Password=%SA_PASSWORD%;TrustServerCertificate=True;Encrypt=False;" easy-db-migrator:profile profile-migrator all || goto :fail
call :done

set STEP=removing PostgreSQL and SQL Server
call :begin
call :remove_the_databases
call :done

set STEP=writing the report
call :begin
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; $text = [IO.File]::ReadAllText($env:SCRIPT); $marker = [char]10 + '#' + 'POWERSHELL' + '#'; $start = $text.IndexOf($marker); if ($start -lt 0) { throw 'the script holds no PowerShell part' }; & ([scriptblock]::Create($text.Substring($start + $marker.Length)))" || goto :fail
call :done

set STEP=keeping the 4 newest performance reports
call :begin
for /f "skip=4 delims=" %%d in ('dir /b /ad /o-n "%REPORTS%\performance_*"') do rmdir /s /q "%REPORTS%\%%d" || goto :fail
call :done

set STEP=opening the report
call :begin
start "" "%RESULTS%\report.html"
call :done

echo.
echo ==========================================
echo  100%% done, all steps passed
echo  report: %RESULTS%\report.html
echo ==========================================
exit /b 0

:begin
set /a STEP_NUMBER+=1
set /a PERCENT=(STEP_NUMBER-1)*100/STEPS
echo.
echo ==========================================
echo  [%STEP_NUMBER%/%STEPS%, %PERCENT%%% done] BEGIN %STEP%
echo ==========================================
exit /b 0

:done
echo ==========================================
echo  END %STEP% - passed
echo ==========================================
exit /b 0

:remove_the_databases
docker rm -f %POSTGRES% %MSSQL% >nul 2>&1
docker network rm %NETWORK% >nul 2>&1
exit /b 0

:nodocker
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
echo.
echo  docker is not running. the migrator is profiled in containers; a profile that
echo  could not run is a failed run, never a skipped check
exit /b 1

:fail
set BUILD_ERROR=%errorlevel%
:fail_with_build_error
if "%BUILD_ERROR%"=="0" set BUILD_ERROR=1
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
call :remove_the_databases
exit /b %BUILD_ERROR%

#POWERSHELL#
$ErrorActionPreference = 'Stop'
$invariant = [Globalization.CultureInfo]::InvariantCulture
$utf8 = New-Object Text.UTF8Encoding $false
$results = $env:RESULTS

$databases = @(
    [pscustomobject]@{ Name = 'postgres'; Title = 'PostgreSQL'; Server = 'PostgreSQL 17' },
    [pscustomobject]@{ Name = 'mssql'; Title = 'SQL Server'; Server = 'SQL Server 2022' }
)
$scriptSets = @(
    [pscustomobject]@{ Name = '100'; Title = '100 scripts'; Scripts = 100; Warmup = 3; Runs = 20 },
    [pscustomobject]@{ Name = '1000'; Title = '1,000 scripts'; Scripts = 1000; Warmup = 1; Runs = 5 },
    [pscustomobject]@{ Name = 'big'; Title = 'one script of 10,000 inserts'; Scripts = 1; Warmup = 1; Runs = 5 }
)
$loads = foreach ($database in $databases) {
    foreach ($scriptSet in $scriptSets) {
        [pscustomobject]@{
            Name = "$($database.Name)-new-$($scriptSet.Name)"
            Title = "$($database.Title), $($scriptSet.Title), new"
            Load = "try_apply_migrations with $($scriptSet.Title) on a new database profile_migrations in $($database.Server). The profile program deletes the database before each run."
            Scripts = $scriptSet.Scripts
            Warmup = $scriptSet.Warmup
        }
        [pscustomobject]@{
            Name = "$($database.Name)-applied-$($scriptSet.Name)"
            Title = "$($database.Title), $($scriptSet.Title), applied"
            Load = "try_apply_migrations with $($scriptSet.Title) when all of it already ran in $($database.Server), so the run skips every script."
            Scripts = $scriptSet.Scripts
            Warmup = $scriptSet.Warmup
        }
    }
}

function ConvertTo-EncodedHtml([string] $text) {
    return [Net.WebUtility]::HtmlEncode($text)
}

function Format-Number([double] $value, [string] $format) {
    return $value.ToString($format, $invariant)
}

function Read-ResultFile([string] $relativePath) {
    $path = Join-Path $results $relativePath
    if (-not (Test-Path -LiteralPath $path)) {
        throw "the profile run wrote no $relativePath"
    }
    return [IO.File]::ReadAllText($path, $utf8)
}

function Read-RunTime([string] $relativePath) {
    $text = (Read-ResultFile $relativePath).Trim()
    return [DateTimeOffset]::Parse($text, $invariant).ToLocalTime().ToString('yyyy-MM-dd HH:mm:ss', $invariant)
}

function Find-Version([string] $text, [string] $pattern, [string] $source) {
    $match = [regex]::Match($text, $pattern, [Text.RegularExpressions.RegexOptions]::Multiline)
    if (-not $match.Success) {
        throw "$source names no version for the pattern $pattern"
    }
    return $match.Groups[1].Value
}

function ConvertTo-RowTable($rows) {
    $html = New-Object Text.StringBuilder
    [void] $html.Append('<div class="scroll"><table class="facts"><tbody>')
    foreach ($row in $rows.GetEnumerator()) {
        [void] $html.Append('<tr><th scope="row">' + (ConvertTo-EncodedHtml $row.Key) + '</th><td>' + $row.Value + '</td></tr>')
    }
    [void] $html.Append('</tbody></table></div>')
    return $html.ToString()
}

function Split-Frame([string] $frame) {
    $match = [regex]::Match($frame, '^0x[0-9a-f]+: (?<name>.+) \((?<file>[^()]+):(?<line>\d+):\d+\)$')
    if (-not $match.Success) {
        return [pscustomobject]@{ Name = ($frame -replace '^0x[0-9a-f]+: ', ''); File = ''; Line = '' }
    }
    return [pscustomobject]@{ Name = $match.Groups['name'].Value; File = $match.Groups['file'].Value; Line = $match.Groups['line'].Value }
}

function ConvertTo-FrameHtml($frame) {
    $html = '<code>' + (ConvertTo-EncodedHtml $frame.Name) + '</code>'
    if ($frame.File) {
        $html += ' <span class="where">' + (ConvertTo-EncodedHtml ($frame.File + ':' + $frame.Line)) + '</span>'
    }
    return $html
}

function ConvertTo-TimingHtml($load, [string] $version) {
    $result = (Read-ResultFile "$($load.Name)/hyperfine.json" | ConvertFrom-Json).results[0]
    $cpuSeconds = $result.user + $result.system
    $waitSeconds = [Math]::Max([double] 0, $result.mean - $cpuSeconds)
    $rows = [ordered]@{
        'Mean time of one run' = (Format-Number ($result.mean * 1000) 'N1') + ' ms, with a standard deviation of ' + (Format-Number ($result.stddev * 1000) 'N1') + ' ms'
        'Median' = (Format-Number ($result.median * 1000) 'N1') + ' ms'
        'Fastest and slowest run' = (Format-Number ($result.min * 1000) 'N1') + ' ms and ' + (Format-Number ($result.max * 1000) 'N1') + ' ms'
        'CPU time of the process' = (Format-Number ($cpuSeconds * 1000) 'N1') + ' ms, ' + (Format-Number ($result.user * 1000) 'N1') + ' ms in user mode and ' + (Format-Number ($result.system * 1000) 'N1') + ' ms in the kernel'
        'Time waiting' = (Format-Number ($waitSeconds * 1000) 'N1') + ' ms, ' + (Format-Number ($waitSeconds / $result.mean) 'P0') + ' of the run, on the database, the network and the disk'
        'Mean time per script' = (Format-Number ($result.mean * 1000 / $load.Scripts) 'N2') + ' ms'
        'Runs' = [string] @($result.times).Count + ' measured runs after ' + $load.Warmup + ' warm-up runs'
    }
    $html = '<h3>Run time</h3>'
    $html += '<p class="tool">hyperfine ' + (ConvertTo-EncodedHtml $version) + ', last run ' + (Read-RunTime "$($load.Name)/timing.time") + '. The time holds the start of the profile program and its connection to the database.</p>'
    $html += ConvertTo-RowTable $rows
    $html += '<p><a href="' + $load.Name + '/hyperfine.json">hyperfine.json</a> holds the time of every run.</p>'
    return $html
}

function ConvertTo-CpuHtml($load, [string] $flamegraphVersion, [string] $perfVersion) {
    $svg = [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $results "$($load.Name)/flamegraph.svg")))
    $html = '<h3>CPU time</h3>'
    $samples = [double] (Read-ResultFile "$($load.Name)/cpu.samples").Trim()
    $html += '<p class="tool">cargo-flamegraph ' + (ConvertTo-EncodedHtml $flamegraphVersion) + ' with perf ' + (ConvertTo-EncodedHtml $perfVersion) + ', last run ' + (Read-RunTime "$($load.Name)/cpu.time") + '. perf recorded ' + (Format-Number $samples 'N0') + ' samples at 4999 Hz over the whole run of the profile program.</p>'
    $html += '<p>Each box is a function. Its width is its share of the CPU samples, and the boxes on top of it are the functions it calls. <a href="' + $load.Name + '/flamegraph.svg">Open the interactive flamegraph</a> to zoom in and search.</p>'
    $html += '<img class="flamegraph" alt="CPU flamegraph of ' + (ConvertTo-EncodedHtml $load.Title) + '" src="data:image/svg+xml;base64,' + $svg + '">'
    $html += ConvertTo-FunctionsHtml $load
    return $html
}

function ConvertTo-FunctionsHtml($load) {
    $self = New-Object 'Collections.Generic.Dictionary[string,double]'
    $inclusive = New-Object 'Collections.Generic.Dictionary[string,double]'
    $total = [double] 0
    foreach ($line in ((Read-ResultFile "$($load.Name)/cpu.folded") -split "`n")) {
        $match = [regex]::Match($line.Trim(), '^(?<stack>.+) (?<samples>\d+)$')
        if (-not $match.Success) {
            continue
        }
        $samples = [double] $match.Groups['samples'].Value
        $frames = $match.Groups['stack'].Value -split ';'
        $total += $samples
        $leaf = $frames[$frames.Count - 1]
        if (-not $self.ContainsKey($leaf)) {
            $self[$leaf] = 0
        }
        $self[$leaf] += $samples
        foreach ($frame in ($frames | Select-Object -Unique)) {
            if (-not $inclusive.ContainsKey($frame)) {
                $inclusive[$frame] = 0
            }
            $inclusive[$frame] += $samples
        }
    }
    if ($total -eq 0) {
        throw "the profile run wrote no samples in $($load.Name)/cpu.folded"
    }
    $html = '<h4>The 15 functions with the most CPU samples of their own</h4>'
    $html += '<div class="scroll"><table class="sites"><thead><tr><th scope="col">#</th><th scope="col">Own share</th><th scope="col">Share with its callees</th><th scope="col">Function</th></tr></thead><tbody>'
    $rank = 0
    foreach ($function in ($self.GetEnumerator() | Sort-Object -Property Value -Descending | Select-Object -First 15)) {
        $rank++
        $html += '<tr><td>' + $rank + '</td><td class="number">' + (Format-Number ($function.Value / $total) 'P1') + '</td><td class="number">' + (Format-Number ($inclusive[$function.Key] / $total) 'P1') + '</td><td><code>' + (ConvertTo-EncodedHtml $function.Key) + '</code></td></tr>'
    }
    $html += '</tbody></table></div>'
    $html += '<p><a href="' + $load.Name + '/cpu.folded">cpu.folded</a> holds every call stack with its samples.</p>'
    return $html
}

function ConvertTo-SitesHtml($dhat, $sites, [string] $property, [string] $title) {
    $html = '<h4>' + (ConvertTo-EncodedHtml $title) + '</h4>'
    $html += '<div class="scroll"><table class="sites"><thead><tr><th scope="col">#</th><th scope="col">Bytes</th><th scope="col">Blocks</th><th scope="col">Most bytes live at once</th><th scope="col">Function</th></tr></thead><tbody>'
    $rank = 0
    foreach ($site in ($sites | Sort-Object -Property $property -Descending | Select-Object -First 10)) {
        $rank++
        $frames = @($site.fs | ForEach-Object { Split-Frame ([string] $dhat.ftbl[$_]) })
        $appFrame = $frames | Where-Object { $_.Name -match '^<?(easy_db_migrator_rust|profile_migrations)::' } | Select-Object -First 1
        $crateFrame = $frames | Where-Object { $_.File -match '^[A-Za-z0-9_-]+-\d+\.\d+\.\d+[^/]*/' } | Select-Object -First 1
        if ($appFrame) {
            $function = ConvertTo-FrameHtml $appFrame
        } elseif ($crateFrame) {
            $function = '<span class="where">no function of the migrator in the call stack, allocated in</span> ' + (ConvertTo-FrameHtml $crateFrame)
        } else {
            $function = '<span class="where">no function of the migrator in the call stack, allocated in</span> ' + (ConvertTo-FrameHtml $frames[0])
        }
        $stack = ($frames | ForEach-Object { '<li>' + (ConvertTo-FrameHtml $_) + '</li>' }) -join ''
        $html += '<tr><td>' + $rank + '</td><td class="number">' + (Format-Number $site.tb 'N0') + '</td><td class="number">' + (Format-Number $site.tbk 'N0') + '</td><td class="number">' + (Format-Number $site.mb 'N0') + '</td><td>' + $function + '<details><summary>call stack of ' + $frames.Count + ' frames</summary><ol>' + $stack + '</ol></details></td></tr>'
    }
    $html += '</tbody></table></div>'
    return $html
}

function ConvertTo-HeapHtml($load, [string] $version) {
    $dhat = Read-ResultFile "$($load.Name)/dhat-heap.json" | ConvertFrom-Json
    $sites = @($dhat.pps)
    $totalBytes = ($sites | Measure-Object -Property tb -Sum).Sum
    $totalBlocks = ($sites | Measure-Object -Property tbk -Sum).Sum
    $peakBytes = ($sites | Measure-Object -Property gb -Sum).Sum
    $peakBlocks = ($sites | Measure-Object -Property gbk -Sum).Sum
    $endBytes = ($sites | Measure-Object -Property eb -Sum).Sum
    $endBlocks = ($sites | Measure-Object -Property ebk -Sum).Sum
    $rows = [ordered]@{
        'Allocated in total' = (Format-Number $totalBytes 'N0') + ' bytes in ' + (Format-Number $totalBlocks 'N0') + ' blocks'
        'Live at the peak' = (Format-Number $peakBytes 'N0') + ' bytes in ' + (Format-Number $peakBlocks 'N0') + ' blocks, after ' + (Format-Number ($dhat.tg / 1000000) 'N2') + ' s'
        'Live at the end' = (Format-Number $endBytes 'N0') + ' bytes in ' + (Format-Number $endBlocks 'N0') + ' blocks, after ' + (Format-Number ($dhat.te / 1000000) 'N2') + ' s'
        'Allocation sites' = (Format-Number $sites.Count 'N0')
    }
    $html = '<h3>Heap</h3>'
    $html += '<p class="tool">dhat ' + (ConvertTo-EncodedHtml $version) + ' through the feature dhat-heap, last run ' + (Read-RunTime "$($load.Name)/memory.time") + '. The data covers the whole run of the profile program.</p>'
    $html += ConvertTo-RowTable $rows
    $html += ConvertTo-SitesHtml $dhat $sites 'tb' 'The 10 allocation sites with the most bytes'
    $html += ConvertTo-SitesHtml $dhat $sites 'tbk' 'The 10 allocation sites with the most blocks'
    $html += '<p><a href="' + $load.Name + '/dhat-heap.json">dhat-heap.json</a> holds every allocation site. Open it in <a href="https://nnethercote.github.io/dh_view/dh_view.html">dh_view</a> to read them all.</p>'
    return $html
}

function Read-LoadNumbers([string] $directory, $load) {
    $hyperfinePath = Join-Path $directory "$($load.Name)/hyperfine.json"
    $dhatPath = Join-Path $directory "$($load.Name)/dhat-heap.json"
    if (-not ((Test-Path -LiteralPath $hyperfinePath) -and (Test-Path -LiteralPath $dhatPath))) {
        return $null
    }
    $result = ([IO.File]::ReadAllText($hyperfinePath, $utf8) | ConvertFrom-Json).results[0]
    $sites = @(([IO.File]::ReadAllText($dhatPath, $utf8) | ConvertFrom-Json).pps)
    return [ordered]@{
        'Mean time of one run (ms)' = $result.mean * 1000
        'CPU time of the process (ms)' = ($result.user + $result.system) * 1000
        'Bytes allocated in total' = [double] ($sites | Measure-Object -Property tb -Sum).Sum
        'Blocks allocated in total' = [double] ($sites | Measure-Object -Property tbk -Sum).Sum
        'Bytes live at the peak' = [double] ($sites | Measure-Object -Property gb -Sum).Sum
    }
}

function ConvertTo-ComparisonHtml($loads) {
    $current = Split-Path -Leaf $results
    $previous = Get-ChildItem -LiteralPath $env:REPORTS -Directory -Filter 'performance_*' |
        Where-Object { ($_.Name -lt $current) -and (Test-Path -LiteralPath (Join-Path $_.FullName 'report.html')) } |
        Sort-Object -Property Name |
        Select-Object -Last 1
    $html = '<section id="comparison"><h2>Compared to the previous run</h2>'
    if (-not $previous) {
        return $html + '<p class="load">This is the first report, so there is no previous run to compare to.</p></section>'
    }
    $html += '<p class="load">Compared to <a href="../' + $previous.Name + '/report.html">' + $previous.Name + '</a>. A negative change is an improvement. The heap numbers repeat from run to run, while the times in Docker move, so trust a small change in time less.</p>'
    $html += '<div class="scroll"><table class="sites"><thead><tr><th scope="col">Load</th><th scope="col">Measure</th><th scope="col">Previous</th><th scope="col">This run</th><th scope="col">Change</th></tr></thead><tbody>'
    foreach ($load in $loads) {
        $before = Read-LoadNumbers $previous.FullName $load
        if (-not $before) {
            $html += '<tr><td>' + (ConvertTo-EncodedHtml $load.Title) + '</td><td colspan="4">not in the previous run</td></tr>'
            continue
        }
        $after = Read-LoadNumbers $results $load
        foreach ($measure in $after.Keys) {
            $change = 'not defined'
            if ($before[$measure] -ne 0) {
                $change = (Format-Number (($after[$measure] - $before[$measure]) * 100 / $before[$measure]) '+0.0;-0.0;0.0') + ' %'
            }
            $html += '<tr><td>' + (ConvertTo-EncodedHtml $load.Title) + '</td><td>' + (ConvertTo-EncodedHtml $measure) + '</td><td class="number">' + (Format-Number $before[$measure] 'N1') + '</td><td class="number">' + (Format-Number $after[$measure] 'N1') + '</td><td class="number">' + $change + '</td></tr>'
        }
    }
    return $html + '</tbody></table></div></section>'
}

Write-Host '  BEGIN reading the tool versions'
try {
    $versions = Read-ResultFile 'versions.txt'
    $hyperfineVersion = Find-Version $versions '^hyperfine (\S+)' 'versions.txt'
    $flamegraphVersion = Find-Version $versions '^flamegraph (\S+)' 'versions.txt'
    $perfVersion = Find-Version $versions '^perf version (\S+)' 'versions.txt'
    $rustVersion = Find-Version $versions '^rustc (\S+)' 'versions.txt'
    $dhatVersion = Find-Version ([IO.File]::ReadAllText((Join-Path (Split-Path $env:SCRIPT) 'Cargo.lock'), $utf8)) 'name = "dhat"\r?\nversion = "([^"]+)"' 'Cargo.lock'
} catch {
    Write-Host '  END reading the tool versions - FAILED'
    throw
}
Write-Host '  END reading the tool versions - passed'
$runStart = [DateTime]::ParseExact($env:RUN_ID, 'yyyyMMdd_HHmmss', $invariant).ToString('yyyy-MM-dd HH:mm:ss', $invariant)

$runRows = [ordered]@{
    'Run started' = $runStart
    'Build' = 'release profile with <code>debug = true</code> and <code>CARGO_PROFILE_RELEASE_DEBUG=true</code>, with the symbols kept, in the image <code>easy-db-migrator:profile</code>'
    'Rust' = 'rustc ' + (ConvertTo-EncodedHtml $rustVersion)
    'Program' = 'the profile program <code>examples/profile_migrations.rs</code>, one run per migration run'
    'Scripts' = '100 and 1,000 scripts that each create one table, and one script that creates a table and inserts 10,000 rows. Each set runs new and already applied, on both databases, so the report shows how the cost grows'
    'Databases' = '<code>postgres:17-alpine</code> and <code>mcr.microsoft.com/mssql/server:2022-CU27-ubuntu-22.04</code>, each in its own container, on one Docker network with the profile container'
    'Run time' = 'hyperfine ' + (ConvertTo-EncodedHtml $hyperfineVersion) + ': 3 warm-up runs and 20 measured runs for the 100 scripts, 1 warm-up run and 5 measured runs for the other loads'
    'CPU time' = 'cargo-flamegraph ' + (ConvertTo-EncodedHtml $flamegraphVersion) + ' with perf ' + (ConvertTo-EncodedHtml $perfVersion) + ': one flamegraph per load'
    'Heap' = 'dhat ' + (ConvertTo-EncodedHtml $dhatVersion) + ' through the feature <code>dhat-heap</code>: one <code>dhat-heap.json</code> per load'
}

$page = New-Object Text.StringBuilder
[void] $page.Append(@'
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Migrator Performance</title>
<link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.2/styles/github-dark.min.css" integrity="sha512-rO+olRTkcf304DQBxSWxln8JXCzTHlKnIdnMUwYvQa9/Jd4cQaNkItIUj6Z4nvW1dqK0SKXLbn9h4KwZTNtAyw==" crossorigin="anonymous" referrerpolicy="no-referrer">
<style>
  :root { --background: #ffffff; --text: #1f2328; --muted: #59636e; --line: #d1d9e0; --panel: #f6f8fa; --link: #0969da; --accent: #bc4c00; }
  @media (prefers-color-scheme: dark) { :root { --background: #0d1117; --text: #e6edf3; --muted: #9198a1; --line: #3d444d; --panel: #151b23; --link: #4493f8; --accent: #f0883e; } }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--background); color: var(--text); font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 1240px; margin: 0 auto; padding: 24px 16px 64px; }
  h1 { font-size: 1.8rem; margin: 0 0 16px; }
  h2 { font-size: 1.4rem; margin: 48px 0 8px; padding-top: 16px; border-top: 2px solid var(--line); }
  h3 { font-size: 1.15rem; margin: 28px 0 4px; }
  h4 { font-size: 1rem; margin: 20px 0 8px; }
  a { color: var(--link); }
  nav ul { display: flex; flex-wrap: wrap; gap: 8px 20px; padding: 0; list-style: none; }
  .tool, .load { color: var(--muted); margin: 4px 0 12px; }
  .scroll { overflow-x: auto; }
  table { border-collapse: collapse; margin: 8px 0; }
  th, td { border: 1px solid var(--line); padding: 6px 10px; text-align: left; vertical-align: top; }
  thead th, .facts th { background: var(--panel); }
  .number { text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
  code { font-family: ui-monospace, "Cascadia Code", Consolas, monospace; font-size: 0.9em; overflow-wrap: anywhere; }
  .where { color: var(--muted); font-size: 0.85em; overflow-wrap: anywhere; }
  details ol { margin: 6px 0; padding-left: 24px; }
  .flamegraph { display: block; width: 100%; height: auto; background: #ffffff; border: 1px solid var(--line); }
  .bottlenecks { padding-left: 0; list-style: none; }
  .bottleneck { margin: 20px 0 36px; }
  .bottleneck > li { margin-bottom: 8px; }
  .cost { color: var(--accent); font-weight: 600; }
  figure.code { margin: 12px 0 0; }
  figure.code figcaption { font-family: ui-monospace, "Cascadia Code", Consolas, monospace; font-size: 0.85rem; color: var(--muted); margin-bottom: 4px; overflow-wrap: anywhere; }
  .listing { display: flex; overflow-x: auto; background: #0d1117; border-radius: 6px; }
  .listing pre { margin: 0; padding: 12px 0; font: 0.85rem/1.5 ui-monospace, "Cascadia Code", Consolas, monospace; }
  .listing .numbers { padding: 12px 10px; color: #6e7681; text-align: right; user-select: none; border-right: 1px solid #30363d; }
  .listing code.hljs { padding: 0 12px; background: transparent; overflow-wrap: normal; font-size: inherit; }
</style>
</head>
<body>
<main>
<h1>Performance report of the migrator</h1>
'@)
[void] $page.Append((ConvertTo-RowTable $runRows))
[void] $page.Append('<nav><ul>')
foreach ($load in $loads) {
    [void] $page.Append('<li><a href="#load-' + $load.Name + '">' + (ConvertTo-EncodedHtml $load.Title) + '</a></li>')
}
[void] $page.Append('<li><a href="#comparison">Compared to the previous run</a></li><li><a href="#bottlenecks">Bottlenecks</a></li></ul></nav>')

foreach ($load in $loads) {
    Write-Host "  BEGIN writing the section $($load.Title)"
    try {
        [void] $page.Append('<section id="load-' + $load.Name + '"><h2>' + (ConvertTo-EncodedHtml $load.Title) + '</h2>')
        [void] $page.Append('<p class="load">Load: ' + (ConvertTo-EncodedHtml $load.Load) + '</p>')
        [void] $page.Append((ConvertTo-TimingHtml $load $hyperfineVersion))
        [void] $page.Append((ConvertTo-CpuHtml $load $flamegraphVersion $perfVersion))
        [void] $page.Append((ConvertTo-HeapHtml $load $dhatVersion))
        [void] $page.Append('</section>')
    } catch {
        Write-Host "  END writing the section $($load.Title) - FAILED"
        throw
    }
    Write-Host "  END writing the section $($load.Title) - passed"
}

Write-Host '  BEGIN writing the comparison'
try {
    [void] $page.Append((ConvertTo-ComparisonHtml $loads))
} catch {
    Write-Host '  END writing the comparison - FAILED'
    throw
}
Write-Host '  END writing the comparison - passed'

[void] $page.Append(@'
<section id="bottlenecks">
<h2>Bottlenecks</h2>
<p id="bottlenecks-waiting">The agent has not analyzed this run yet. After every run it writes one point per bottleneck here, with the CPU time or the memory it takes, a tip, and the code.</p>
</section>
</main>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.2/highlight.min.js" integrity="sha512-VSPLUv/n1Bmn+4zoxBNwpuFAO3//79I0Aax/qHDx24R47vylPcc9PrHDCqlePwHnh3joiM7/YTQhcXyQAAxvPQ==" crossorigin="anonymous" referrerpolicy="no-referrer"></script>
<script>hljs.highlightAll();</script>
</body>
</html>
'@)

[IO.File]::WriteAllText((Join-Path $results 'report.html'), $page.ToString(), $utf8)
