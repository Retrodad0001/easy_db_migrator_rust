@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

set STEP=checking docker
call :begin
docker info >nul 2>&1 || goto :nodocker
call :done

set STEP=building the test binaries first
call :begin
REM the tests run in the dev profile, without --release, on purpose:
REM the demo build is faster
cargo test -p easy_db_migrator_rust --all-targets --all-features --no-run || goto :fail
call :done

echo.
echo  this library has no user interface, so this run shows no window. It runs
echo  every test of the project against real databases in containers.

set SUITES=0
set FAILED=0

for %%f in (tests\*.rs) do (
    set /a SUITES+=1
    echo.
    echo ==========================================
    echo  BEGIN suite %%~nf
    echo ==========================================
    cargo test -p easy_db_migrator_rust --test %%~nf --all-features -- --nocapture
    if errorlevel 1 (
        set /a FAILED+=1
        echo ==========================================
        echo  END suite %%~nf - FAILED
        echo ==========================================
    ) else (
        echo ==========================================
        echo  END suite %%~nf - passed
        echo ==========================================
    )
)

if %SUITES%==0 goto :nosuites

set STEP=the doc tests
call :begin
cargo test -p easy_db_migrator_rust --doc --all-features || goto :fail
call :done

echo.
echo ==========================================
if %FAILED%==0 (
    echo  all %SUITES% suite^(s^) passed
    echo ==========================================
    exit /b 0
)
echo  %FAILED% of %SUITES% suite^(s^) FAILED
echo ==========================================
exit /b 1

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

:nosuites
echo.
echo ==========================================
echo  FAILED - no suites found in tests\
echo ==========================================
echo  a run that executed zero tests is not evidence that anything works
exit /b 1

:nodocker
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
echo.
echo  docker is not running. a container that would not start is a failed run,
echo  never a skipped check
exit /b 1

:fail
set BUILD_ERROR=%errorlevel%
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
exit /b %BUILD_ERROR%
