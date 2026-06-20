@echo off
REM ============================================================================
REM  1-verify-console.bat
REM  Run this on the GAMING PC (the machine that runs Wreckfest 1).
REM  It prints live telemetry so you can confirm wreckfest-teleport is reading
REM  REAL data while you drive (speed, position, g-force, yaw rate, impact).
REM
REM  Reading another process's memory needs Administrator rights, so this script
REM  re-launches itself elevated automatically (accept the UAC prompt).
REM ============================================================================

REM --- self-elevate if not already admin ---
net session >nul 2>&1
if %errorlevel% neq 0 (
    powershell -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
    exit /b
)

cd /d "%~dp0"
echo Starting wreckfest-teleport in console mode...
echo Launch Wreckfest, start a race, and watch the values move.
echo Press Ctrl+C to stop.
echo.
wreckfest-teleport.exe console --rate 100

echo.
echo wreckfest-teleport stopped.
pause
