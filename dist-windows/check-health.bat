@echo off
setlocal
set POSTERN_HOME=%USERPROFILE%\.postern
set POSTERN_CONTROL_TOKEN=%POSTERN_HOME%\control.token
set POSTERN_CONTROL_PORT=127.0.0.1:7878
echo querying daemon health ...
"%~dp0postern.exe" daemon status
pause
