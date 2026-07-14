@echo on
setlocal

set "VERSION="
for /f "tokens=3" %%A in ('findstr /b /c:"version = " rust\Cargo.toml') do (
  if not defined VERSION set "VERSION=%%~A"
)

if not defined VERSION (
  echo Failed to determine version from rust\Cargo.toml
  exit /b 1
)

set "ASSETS_DIR=C:\code\Ceiling-release\assets"
set "MISSING=0"

for %%F in (
  "Ceiling-%VERSION%-Setup.exe"
  "Ceiling-%VERSION%-Setup.exe.sha256"
  "Ceiling-%VERSION%-portable.exe"
  "Ceiling-%VERSION%-portable.exe.sha256"
) do (
  if not exist "%ASSETS_DIR%\%%~F" (
    echo Missing release artifact: %ASSETS_DIR%\%%~F
    set "MISSING=1"
  ) else (
    echo Found %ASSETS_DIR%\%%~F
  )
)

if "%MISSING%"=="1" exit /b 1
exit /b 0
