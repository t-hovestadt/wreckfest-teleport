@echo off
REM ============================================================================
REM  3-stream-simhub-local.bat
REM  Run this on the GAMING PC when SimHub runs ON THE SAME PC (easiest first
REM  test). Streams telemetry to 127.0.0.1 in the Codemasters extradata=3
REM  (DiRT Rally 2.0) format. In SimHub, enable the DiRT Rally 2.0 game
REM  (UDP port 20777). Good for validating wind sim + bass shakers.
REM  Use 1-verify-console.bat first to confirm real data is being read.
REM
REM  Reading game memory needs Administrator rights; this self-elevates.
REM ============================================================================

set TARGET=127.0.0.1:20777

REM --- self-elevate if not already admin ---
net session >nul 2>&1
if %errorlevel% neq 0 (
    powershell -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

cd /d "%~dp0"
echo Streaming wreckfest-teleport (DiRT Rally 2.0 format) to %TARGET% ...
echo Press Ctrl+C to stop.
echo.
wreckfest-teleport.exe udp --target %TARGET% --rate 100 --format simhub

echo.
echo wreckfest-teleport stopped.
pause
