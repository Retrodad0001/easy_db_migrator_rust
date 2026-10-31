@echo off
setlocal
cd /d "%~dp0"

for /f %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set RUN_ID=%%t
set REPORTS=%~dp0reports
set RESULTS=%REPORTS%\validate_%RUN_ID%
set SCRIPT=%~f0
set PACKAGES=-p easy_db_migrator_rust
set FEATURES=--features postgres,mssql
set "FILTER=(tests[\\/])"
set STEPS=13
set STEP_NUMBER=0
set FAILED=0

mkdir "%RESULTS%" || exit /b 1

set STEP=checking docker
set "COMMAND=docker info >nul"
call :run_step
if %STEP_FAILED%==1 goto :nodocker

set STEP=format
set "COMMAND=cargo fmt %PACKAGES% --check"
call :run_step

set STEP=markdownlint
set "COMMAND=docker run --rm -v "%CD%:/workdir" davidanson/markdownlint-cli2:latest "**/*.md" "#target/**""
call :run_step

set STEP=spelling of the README
set "COMMAND=typos --locale en-us README.md"
call :run_step

set STEP=semgrep
set "COMMAND=docker run --rm -v "%CD%:/src" semgrep/semgrep semgrep scan --config=auto --exclude=target --exclude=reports --error --quiet /src"
call :run_step

set STEP=clippy
REM --release on purpose: the tests and the image build in release, and a warning
REM that only the release build raises must fail this step too
set "COMMAND=cargo clippy %PACKAGES% --all-targets --release %FEATURES% -- -D warnings"
call :run_step

set STEP=cargo audit
set "COMMAND=cargo audit --deny warnings"
call :run_step

set STEP=unit tests
set "COMMAND=cargo test %PACKAGES% --release --lib %FEATURES%"
call :run_step

set STEP=integration tests
set "COMMAND=cargo test %PACKAGES% --release --test * %FEATURES%"
call :run_step

set STEP=doc tests
set "COMMAND=cargo test %PACKAGES% --release --doc %FEATURES%"
call :run_step

set STEP=cleaning the coverage data
set "COMMAND=cargo llvm-cov clean --workspace"
call :run_step

set STEP=coverage of the integration tests
REM this measure has no --release on purpose: coverage needs an instrumented build,
REM and an optimized build inlines code so its line numbers stop matching the source
set "COMMAND=cargo llvm-cov %PACKAGES% --all-features --test integration_tests --ignore-filename-regex "%FILTER%" --show-missing-lines"
call :run_step

set STEP=writing the coverage report
set "COMMAND=cargo llvm-cov report %PACKAGES% --ignore-filename-regex "%FILTER%" --html --output-dir "%RESULTS%\coverage""
call :run_step

goto :finish

:nodocker
echo.
echo  docker is not running. the integration suites need testcontainers; a
echo  suite that could not start is a failed run, never a skipped check

:finish
set ACTION=report
call :run_the_powershell_part || set FAILED=1

for /f "skip=4 delims=" %%d in ('dir /b /ad /o-n "%REPORTS%\validate_*"') do rmdir /s /q "%REPORTS%\%%d"

start "" "%RESULTS%\report.html"

echo.
echo ==========================================
if %FAILED%==0 (echo  100%% done, all steps passed) else (echo  100%% done, a step FAILED)
echo  report: %RESULTS%\report.html
echo ==========================================
exit /b %FAILED%

:run_step
set /a STEP_NUMBER+=1
set /a PERCENT=(STEP_NUMBER-1)*100/STEPS
echo.
echo ==========================================
echo  [%STEP_NUMBER%/%STEPS%, %PERCENT%%% done] BEGIN %STEP%
echo ==========================================
set ACTION=step
set STEP_FAILED=0
call :run_the_powershell_part
if errorlevel 1 set STEP_FAILED=1
if %STEP_FAILED%==1 set FAILED=1
echo ==========================================
if %STEP_FAILED%==1 (echo  END %STEP% - FAILED) else (echo  END %STEP% - passed)
echo ==========================================
exit /b 0

:run_the_powershell_part
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; $text = [IO.File]::ReadAllText($env:SCRIPT); $marker = [char]10 + '#' + 'POWERSHELL' + '#'; $start = $text.IndexOf($marker); if ($start -lt 0) { throw 'the script holds no PowerShell part' }; & ([scriptblock]::Create($text.Substring($start + $marker.Length)))"
exit /b %errorlevel%

#POWERSHELL#
$ErrorActionPreference = 'Stop'
$invariant = [Globalization.CultureInfo]::InvariantCulture
$utf8 = New-Object Text.UTF8Encoding $false
$results = $env:RESULTS
$stepsFile = Join-Path $results 'steps.txt'

function ConvertTo-EncodedHtml([string] $text) {
    return [Net.WebUtility]::HtmlEncode($text)
}

function Invoke-Step {
    $log = Join-Path $results "step_$($env:STEP_NUMBER).log"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    & cmd.exe /d /c "$env:COMMAND 2>&1" | Tee-Object -FilePath $log
    $exitCode = $LASTEXITCODE
    $seconds = $watch.Elapsed.TotalSeconds.ToString('N1', $invariant)
    $status = if ($exitCode -eq 0) { 'ok' } else { 'not ok' }
    $line = @($env:STEP_NUMBER, $env:STEP, $status, $seconds, $env:COMMAND) -join "`t"
    [IO.File]::AppendAllText($stepsFile, $line + "`n", $utf8)
    exit $exitCode
}

function Read-Log([string] $number) {
    $path = Join-Path $results "step_$number.log"
    if (-not (Test-Path -LiteralPath $path)) {
        return ''
    }
    return [IO.File]::ReadAllText($path)
}

function Read-FailureOutputs([string] $log) {
    $outputs = @{}
    $pattern = '(?ms)^---- (?<name>.+?) stdout ----\r?$(?<output>.*?)(?=^---- |^failures:)'
    foreach ($match in [regex]::Matches($log, $pattern)) {
        $outputs[$match.Groups['name'].Value] = $match.Groups['output'].Value.Trim()
    }
    return $outputs
}

function Read-Tests($step) {
    $log = Read-Log $step.Number
    $outputs = Read-FailureOutputs $log
    $pattern = '(?m)^test (?<name>.+?) \.\.\. (?<result>ok|FAILED|ignored)\b.*$'
    foreach ($match in [regex]::Matches($log, $pattern)) {
        $name = $match.Groups['name'].Value
        $result = switch ($match.Groups['result'].Value) {
            'ok' { 'ok' }
            'FAILED' { 'not ok' }
            default { 'ignored' }
        }
        [pscustomobject]@{ Suite = $step.Name; Name = $name; Result = $result; Output = $outputs[$name] }
    }
}

function ConvertTo-ResultHtml([string] $result) {
    $class = switch ($result) {
        'ok' { 'ok' }
        'not ok' { 'notok' }
        default { 'other' }
    }
    return '<span class="' + $class + '">' + (ConvertTo-EncodedHtml $result) + '</span>'
}

function ConvertTo-CoverageHtml($steps) {
    $html = '<h2>Coverage</h2>'
    foreach ($step in $steps) {
        $total = [regex]::Match((Read-Log $step.Number), '(?m)^TOTAL\s+\d+\s+\d+\s+(?<regions>[\d.]+%)\s+\d+\s+\d+\s+(?<functions>[\d.]+%)\s+\d+\s+\d+\s+(?<lines>[\d.]+%)')
        if ($total.Success) {
            $html += '<p>' + $total.Groups['lines'].Value + ' of the lines, ' + $total.Groups['functions'].Value + ' of the functions and ' + $total.Groups['regions'].Value + ' of the regions. '
            if (Test-Path -LiteralPath (Join-Path $results 'coverage/html/index.html')) {
                $html += '<a href="coverage/html/index.html">Open the coverage report</a> to read it per file.'
            }
            return $html + '</p>'
        }
    }
    return $html + '<p>' + (ConvertTo-ResultHtml 'not ok') + ' the run measured no coverage.</p>'
}

function Write-Report {
    $steps = @()
    if (Test-Path -LiteralPath $stepsFile) {
        $steps = @([IO.File]::ReadAllLines($stepsFile, $utf8) | ForEach-Object {
            $parts = $_ -split "`t", 5
            [pscustomobject]@{ Number = $parts[0]; Name = $parts[1]; Result = $parts[2]; Seconds = $parts[3]; Command = $parts[4] }
        })
    }
    $tests = @($steps | ForEach-Object { Read-Tests $_ })
    $failedSteps = @($steps | Where-Object { $_.Result -ne 'ok' }).Count
    $failedTests = @($tests | Where-Object { $_.Result -eq 'not ok' }).Count
    $notRun = [int] $env:STEPS - $steps.Count
    $passed = ($failedSteps -eq 0) -and ($notRun -eq 0)
    $runStart = [DateTime]::ParseExact($env:RUN_ID, 'yyyyMMdd_HHmmss', $invariant).ToString('yyyy-MM-dd HH:mm:ss', $invariant)

    $page = New-Object Text.StringBuilder
    [void] $page.Append(@'
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Migrator Validation</title>
<style>
  :root { --background: #ffffff; --text: #1f2328; --muted: #59636e; --line: #d1d9e0; --panel: #f6f8fa; --ok: #1a7f37; --notok: #cf222e; }
  @media (prefers-color-scheme: dark) { :root { --background: #0d1117; --text: #e6edf3; --muted: #9198a1; --line: #3d444d; --panel: #151b23; --ok: #3fb950; --notok: #f85149; } }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--background); color: var(--text); font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 1240px; margin: 0 auto; padding: 24px 16px 64px; }
  h1 { font-size: 1.8rem; margin: 0 0 16px; }
  h2 { font-size: 1.4rem; margin: 40px 0 8px; padding-top: 16px; border-top: 2px solid var(--line); }
  .scroll { overflow-x: auto; }
  table { border-collapse: collapse; margin: 8px 0; width: 100%; }
  th, td { border: 1px solid var(--line); padding: 6px 10px; text-align: left; vertical-align: top; }
  thead th { background: var(--panel); }
  .number { text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
  code { font-family: ui-monospace, "Cascadia Code", Consolas, monospace; font-size: 0.9em; overflow-wrap: anywhere; }
  pre { margin: 6px 0; padding: 10px; overflow-x: auto; background: var(--panel); font: 0.85rem/1.4 ui-monospace, "Cascadia Code", Consolas, monospace; }
  .ok { color: var(--ok); font-weight: 600; }
  .notok { color: var(--notok); font-weight: 600; }
  .other { color: var(--muted); font-weight: 600; }
  .summary { font-size: 1.15rem; margin: 0 0 4px; }
  .muted { color: var(--muted); margin: 0 0 12px; }
</style>
</head>
<body>
<main>
<h1>Validation report of the migrator</h1>
'@)
    $verdict = if ($passed) { 'ok' } else { 'not ok' }
    [void] $page.Append('<p class="summary">Result: ' + (ConvertTo-ResultHtml $verdict) + '</p>')
    [void] $page.Append('<p class="muted">Run started ' + $runStart + '. ' + ($steps.Count - $failedSteps) + ' of ' + $env:STEPS + ' steps ok. ' + ($tests.Count - $failedTests) + ' of ' + $tests.Count + ' tests ok.</p>')

    [void] $page.Append('<h2>Steps</h2><div class="scroll"><table><thead><tr><th scope="col">#</th><th scope="col">Step</th><th scope="col">Result</th><th scope="col">Time</th><th scope="col">Command and output</th></tr></thead><tbody>')
    foreach ($step in $steps) {
        $open = if ($step.Result -eq 'ok') { '' } else { ' open' }
        $output = ConvertTo-EncodedHtml (Read-Log $step.Number)
        [void] $page.Append('<tr><td>' + $step.Number + '</td><td>' + (ConvertTo-EncodedHtml $step.Name) + '</td><td>' + (ConvertTo-ResultHtml $step.Result) + '</td><td class="number">' + $step.Seconds + ' s</td><td><code>' + (ConvertTo-EncodedHtml $step.Command) + '</code><details' + $open + '><summary>output</summary><pre>' + $output + '</pre></details></td></tr>')
    }
    if ($notRun -gt 0) {
        [void] $page.Append('<tr><td colspan="5">' + (ConvertTo-ResultHtml 'not ok') + ' ' + $notRun + ' steps did not run, because docker is not running</td></tr>')
    }
    [void] $page.Append('</tbody></table></div>')

    [void] $page.Append((ConvertTo-CoverageHtml $steps))

    if ($failedTests -gt 0) {
        [void] $page.Append('<h2>Failed tests</h2>')
        foreach ($test in ($tests | Where-Object { $_.Result -eq 'not ok' })) {
            [void] $page.Append('<details open><summary><code>' + (ConvertTo-EncodedHtml $test.Name) + '</code></summary><pre>' + (ConvertTo-EncodedHtml $test.Output) + '</pre></details>')
        }
    }

    [void] $page.Append('<h2>All tests</h2><div class="scroll"><table><thead><tr><th scope="col">#</th><th scope="col">Suite</th><th scope="col">Test</th><th scope="col">Result</th></tr></thead><tbody>')
    $rank = 0
    foreach ($test in $tests) {
        $rank++
        [void] $page.Append('<tr><td>' + $rank + '</td><td>' + (ConvertTo-EncodedHtml $test.Suite) + '</td><td><code>' + (ConvertTo-EncodedHtml $test.Name) + '</code></td><td>' + (ConvertTo-ResultHtml $test.Result) + '</td></tr>')
    }
    [void] $page.Append('</tbody></table></div></main></body></html>')

    [IO.File]::WriteAllText((Join-Path $results 'report.html'), $page.ToString(), $utf8)
}

switch ($env:ACTION) {
    'step' { Invoke-Step }
    'report' { Write-Report }
    default { throw "no action named $($env:ACTION)" }
}
