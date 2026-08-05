@echo off
setlocal
cd /d "%~dp0"

rem Default bind is 127.0.0.1:17843 — open the site shortly after start.
start "" cmd /c "timeout /t 1 /nobreak >nul & start http://127.0.0.1:17843/"

"%~dp0bigshot-voter.exe" %*
set EXITCODE=%ERRORLEVEL%
if not %EXITCODE%==0 (
  echo.
  echo Server exited with code %EXITCODE%.
  pause
)
exit /b %EXITCODE%
