@echo off
REM Build Windows installer: load MSVC x64 env -> frontend build -> Rust release -> NSIS.
REM Called by the VSCode task to avoid PowerShell -> cmd /c quote nesting issues.
REM NOTE: Keep this file pure ASCII. Windows cmd.exe reads .cmd files with the
REM system ANSI code page (GBK here); non-ASCII bytes get garbled and may be
REM parsed as commands. Do not add Chinese/comments with non-ASCII characters.
setlocal

REM Add WorkBuddy's bundled Node.js to PATH (no system-wide Node installed).
set "PATH=%USERPROFILE%\.workbuddy\binaries\node\versions\22.22.2-6;%PATH%"

REM Load MSVC x64 build environment (real link.exe + INCLUDE/LIB).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 (
  echo [ERROR] Failed to initialize MSVC environment.
  exit /b 1
)

cd /d "%~dp0..\desktop"
if errorlevel 1 exit /b 1

echo [INFO] node:
call node --version
echo [INFO] npm:
call npm --version
call npm run build
exit /b %errorlevel%
