@echo off
setlocal
cd /d "%~dp0"

for %%i in ("%~dp0..") do set WORKSPACE=%%~fi
for /f %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set RUN_ID=%%t
set REPORTS=%~dp0reports
set RESULTS=%REPORTS%\performance_%RUN_ID%
set SCRIPT=%~f0
set CARGO_PROFILE_RELEASE_DEBUG=true
set NETWORK=easy-db-migrator-profile
set POSTGRES=easy-db-migrator-profile-postgres
set MSSQL=easy-db-migrator-profile-mssql
set SA_PASSWORD=yourStrong(!)Password

set STEP=checking docker
call :begin
docker info >nul 2>&1 || goto :nodocker
call :done

set STEP=building the profile image in release mode with debug information
call :begin
docker build -f Dockerfile --target profile --build-arg CARGO_PROFILE_RELEASE_DEBUG -t easy-db-migrator:profile .. || goto :fail
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
echo  all steps passed
echo  report: %RESULTS%\report.html
echo ==========================================
exit /b 0

:begin
echo.
echo ==========================================
echo  BEGIN %STEP%
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
$scripts = 100

$loads = @(
    [pscustomobject]@{
        Name = 'postgres-new'
        Title = 'PostgreSQL, 100 new scripts'
        Load = 'try_apply_migrations with 100 new scripts on a new database profile_migrations in PostgreSQL 17. The profile program deletes the database before each run.'
    },
    [pscustomobject]@{
        Name = 'postgres-applied'
        Title = 'PostgreSQL, 100 applied scripts'
        Load = 'try_apply_migrations with the same 100 scripts when all 100 already ran in PostgreSQL 17, so the run skips every script.'
    },
    [pscustomobject]@{
        Name = 'mssql-new'
        Title = 'SQL Server, 100 new scripts'
        Load = 'try_apply_migrations with 100 new scripts on a new database profile_migrations in SQL Server 2022. The profile program deletes the database before each run.'
    },
    [pscustomobject]@{
        Name = 'mssql-applied'
        Title = 'SQL Server, 100 applied scripts'
        Load = 'try_apply_migrations with the same 100 scripts when all 100 already ran in SQL Server 2022, so the run skips every script.'
    }
)

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
    $rows = [ordered]@{
        'Mean time of one run' = (Format-Number ($result.mean * 1000) 'N1') + ' ms, with a standard deviation of ' + (Format-Number ($result.stddev * 1000) 'N1') + ' ms'
        'Median' = (Format-Number ($result.median * 1000) 'N1') + ' ms'
        'Fastest and slowest run' = (Format-Number ($result.min * 1000) 'N1') + ' ms and ' + (Format-Number ($result.max * 1000) 'N1') + ' ms'
        'Mean time per script' = (Format-Number ($result.mean * 1000 / $scripts) 'N2') + ' ms'
        'Runs' = [string] @($result.times).Count + ' measured runs after 3 warm-up runs'
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
    $html += '<h4>The 10 allocation sites with the most bytes</h4>'
    $html += '<div class="scroll"><table class="sites"><thead><tr><th scope="col">#</th><th scope="col">Bytes</th><th scope="col">Blocks</th><th scope="col">Most bytes live at once</th><th scope="col">Function</th></tr></thead><tbody>'
    $rank = 0
    foreach ($site in ($sites | Sort-Object -Property tb -Descending | Select-Object -First 10)) {
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
    $html += '<p><a href="' + $load.Name + '/dhat-heap.json">dhat-heap.json</a> holds every allocation site. Open it in <a href="https://nnethercote.github.io/dh_view/dh_view.html">dh_view</a> to read them all.</p>'
    return $html
}

Write-Host '  BEGIN reading the tool versions'
try {
    $versions = Read-ResultFile 'versions.txt'
    $hyperfineVersion = Find-Version $versions '^hyperfine (\S+)' 'versions.txt'
    $flamegraphVersion = Find-Version $versions '^flamegraph (\S+)' 'versions.txt'
    $perfVersion = Find-Version $versions '^perf version (\S+)' 'versions.txt'
    $rustVersion = Find-Version $versions '^rustc (\S+)' 'versions.txt'
    $dhatVersion = Find-Version ([IO.File]::ReadAllText((Join-Path $env:WORKSPACE 'Cargo.lock'), $utf8)) 'name = "dhat"\r?\nversion = "([^"]+)"' 'Cargo.lock'
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
    'Scripts' = '100 scripts, <code>20260101_001_create_table_001.sql</code> to <code>20260101_100_create_table_100.sql</code>, each creates one table'
    'Databases' = '<code>postgres:17-alpine</code> and <code>mcr.microsoft.com/mssql/server:2022-CU27-ubuntu-22.04</code>, each in its own container, on one Docker network with the profile container'
    'Run time' = 'hyperfine ' + (ConvertTo-EncodedHtml $hyperfineVersion) + ': 3 warm-up runs and 20 measured runs per load'
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
[void] $page.Append('<li><a href="#bottlenecks">Bottlenecks</a></li></ul></nav>')

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
