@echo off
setlocal
cd /d "%~dp0"

for %%i in ("%~dp0..") do set WORKSPACE=%%~fi
set PROJECT=easy_db_migrator_rust
set PACKAGES=-p easy_db_migrator_rust
set FEATURES=--features postgres,mssql

set STEP=checking docker
call :begin
docker info >nul 2>&1 || goto :nodocker
call :done

set STEP=format
call :begin
cargo fmt %PACKAGES% --check || goto :fail
call :done

set STEP=markdownlint
call :begin
docker run --rm -v "%WORKSPACE%:/workdir" davidanson/markdownlint-cli2:latest "%PROJECT%/**/*.md" "#**/target/**" || goto :fail
call :done

set STEP=semgrep
call :begin
docker run --rm -v "%CD%:/src" semgrep/semgrep semgrep scan --config=auto --exclude=target --error --quiet /src || goto :fail
call :done

set STEP=clippy
call :begin
REM --release on purpose: the tests and the image build in release, and a warning
REM that only the release build raises must fail this step too
cargo clippy %PACKAGES% --all-targets --release %FEATURES% -- -D warnings || goto :fail
call :done

set STEP=unit tests
call :begin
cargo test %PACKAGES% --release --lib %FEATURES% || goto :fail
call :done

set STEP=integration tests
call :begin
cargo test %PACKAGES% --release --test * %FEATURES% || goto :fail
call :done

echo.
echo ==========================================
echo  all steps passed
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
echo  docker is not running. the integration suites need testcontainers; a
echo  suite that could not start is a failed run, never a skipped check
exit /b 1

:fail
set BUILD_ERROR=%errorlevel%
echo ==========================================
echo  END %STEP% - FAILED
echo ==========================================
exit /b %BUILD_ERROR%
