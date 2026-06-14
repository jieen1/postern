@echo off
setlocal
set POSTERN_HOME=%USERPROFILE%\.postern
if not exist "%POSTERN_HOME%" mkdir "%POSTERN_HOME%"
set POSTERN_DB=%POSTERN_HOME%\policy.db
set POSTERN_VAULT=%POSTERN_HOME%\vault.postern
set POSTERN_KEYFILE=%POSTERN_HOME%\key
set POSTERN_CONTROL_TOKEN=%POSTERN_HOME%\control.token
set POSTERN_CONTROL_PORT=127.0.0.1:7878
set POSTERN_DATA_PORT=127.0.0.1:7879
if not exist "%POSTERN_CONTROL_TOKEN%" (
  echo [first run] initializing %POSTERN_HOME% ...
  "%~dp0posternd.exe" init || (echo init failed & pause & exit /b 1)
)
echo daemon listening on %POSTERN_CONTROL_PORT% (control) / %POSTERN_DATA_PORT% (data)
echo control-token: %POSTERN_CONTROL_TOKEN%
"%~dp0posternd.exe" run
