@echo off
setlocal
cd /d "%~dp0"

for /f %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set RUN_ID=%%t
set REPORTS=%~dp0reports
set REPORT=%REPORTS%\coverage_%RUN_ID%
set "FILTER=(tests[\\/])"

set STEP=checking docker
call :begin
docker info >nul 2>&1 || goto :nodocker
call :done

set STEP=coverage of the integration tests
call :begin
cargo llvm-cov clean --workspace || goto :fail
REM this measure has no --release on purpose: coverage needs an instrumented build,
REM and an optimised build inlines code so its line numbers stop matching the source
cargo llvm-cov -p easy_db_migrator_rust --all-features --test integration_tests --ignore-filename-regex "%FILTER%" --show-missing-lines || goto :fail
cargo llvm-cov report -p easy_db_migrator_rust --ignore-filename-regex "%FILTER%" --html --output-dir "%REPORT%" || goto :fail
call :done

set STEP=keeping the 4 newest coverage reports
call :begin
for /f "skip=4 delims=" %%d in ('dir /b /ad /o-n "%REPORTS%\coverage_*"') do rmdir /s /q "%REPORTS%\%%d" || goto :fail
call :done

set STEP=opening the report
call :begin
start "" "%REPORT%\html\index.html"
call :done

echo.
echo ==========================================
echo  all steps passed
echo  report: %REPORT%\html\index.html
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

:nodocker
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
echo.
echo  docker is not running. the integration tests need testcontainers; a measure that
echo  could not run is a failed check, never a skipped one
exit /b 1

:fail
set BUILD_ERROR=%errorlevel%
:fail_with_build_error
if "%BUILD_ERROR%"=="0" set BUILD_ERROR=1
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
exit /b %BUILD_ERROR%
