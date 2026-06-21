@echo off
REM ============================================================================
REM  4-pick.bat
REM  Run this on the GAMING PC for MULTIPLAYER. Opens a small window listing
REM  every car with a live speed; click the car whose speed matches you (brake
REM  and it drops to 0) to stream it. Single-player auto-selects car 00, so you
REM  can ignore the window there.
REM  Streams the Codemasters extradata=3 (DiRT Rally 2.0) format to the SimHub
REM  PC; enable the DiRT Rally 2.0 game in SimHub (UDP 20777).
REM
REM  Edit the IP below if your SimHub PC differs. For a single-PC test, set
REM  TARGET to 127.0.0.1:20777.
REM  Reading game memory needs Administrator rights; this self-elevates.
REM ============================================================================

set TARGET=192.168.50.2:20777

net session >nul 2>&1
if %errorlevel% neq 0 (
    powershell -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

cd /d "%~dp0"
echo Opening car picker, streaming to %TARGET% ...
echo Close the picker window to stop.
echo.
wreckfest-teleport.exe pick --target %TARGET%

echo.
echo wreckfest-teleport stopped.
pause
