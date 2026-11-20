@echo off
REM FAST iteration build: Rust release compile only.
REM Skips npm install, vite build, NSIS installer and zip on purpose.
REM Use this when you only changed Rust source and just want to RUN the app
REM to verify behavior (e.g. testing a book source). The portable exe stores
REM its data in a data\ folder next to the exe, isolated from the installed app.
REM
REM For a full installer, use the normal build task (build-windows.cmd).
setlocal

REM Load MSVC x64 build environment (real link.exe + INCLUDE/LIB).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 (
  echo [ERROR] Failed to initialize MSVC environment.
  exit /b 1
)

cd /d "%~dp0.."
if errorlevel 1 exit /b 1

echo [INFO] Building reader-desktop (release, incremental)...
call cargo build --release -p reader-desktop
if errorlevel 1 (
  echo [ERROR] cargo build failed.
  exit /b 1
)

if not exist "desktop\dist-portable" mkdir "desktop\dist-portable"
copy /Y "target\release\reader-desktop.exe" "desktop\dist-portable\Reader.exe" >nul
if errorlevel 1 exit /b 1

echo.
echo [DONE] Portable exe: desktop\dist-portable\Reader.exe
echo         Run it directly to verify. A data\ folder is created on first launch.
echo         To make an installer, run the normal build task instead.
exit /b 0
