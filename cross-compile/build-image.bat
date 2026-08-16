@echo off
:: Build the pluto73-cross image. Run once (and again only if the Dockerfile changes).
::
:: The Linaro toolchain tarball is downloaded automatically into cross-compile\ on first run (checksum-verified) from ARM's Legacy Linaro GNU Toolchains archive; alternatively pass its path as the first argument.
setlocal enabledelayedexpansion

set HERE=%~dp0
for %%I in ("%HERE%..") do set APP_DIR=%%~fI
cd /d "%APP_DIR%"

if "%~1"=="" (
    set TARBALL=%HERE%gcc-linaro-7.5.0-2019.12-x86_64_arm-linux-gnueabihf.tar.xz
) else (
    set TARBALL=%~1
)

set TARBALL_URL=https://developer.arm.com/-/cdn-downloads/permalink/legacy-linaro-gnu-toolchains/7.5-2019.12/gcc-linaro-7.5.0-2019.12-x86_64_arm-linux-gnueabihf.tar.xz
set TARBALL_SHA256=abf877f021c5f094d396bac4d842ed6f13aecbf4c477fc5825cf2d8b1fe3ef22

if not exist "%TARBALL%" (
    echo ==> toolchain tarball not found, downloading 105 MB to %TARBALL%
    curl -fL -o "%TARBALL%.part" "%TARBALL_URL%"
    if errorlevel 1 (
        echo error: failed to download toolchain tarball >&2
        exit /b 1
    )
    move /y "%TARBALL%.part" "%TARBALL%" >nul
)

for /f "usebackq tokens=*" %%H in (`powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 '%TARBALL%').Hash.ToLower()"`) do set ACTUAL_SHA256=%%H

if not "%ACTUAL_SHA256%"=="%TARBALL_SHA256%" (
    echo error: checksum mismatch for %TARBALL% >&2
    echo   expected %TARBALL_SHA256% >&2
    echo   got      %ACTUAL_SHA256% >&2
    echo delete the file and re-run to re-download, or see README.md >&2
    exit /b 1
)

for /f "delims=" %%F in ("%TARBALL%") do set TARBALL_NAME=%%~nxF

set CTX=%TEMP%\pluto73_docker_build_%RANDOM%
mkdir "%CTX%"
copy /y "%HERE%Dockerfile" "%CTX%\" >nul
xcopy /s /i /y "%HERE%libs" "%CTX%\libs" >nul
copy /y "%TARBALL%" "%CTX%\%TARBALL_NAME%" >nul

echo ==> Building docker image pluto73-cross:latest...
docker build --platform linux/amd64 --build-arg "TOOLCHAIN_TARBALL=%TARBALL_NAME%" -t pluto73-cross:latest "%CTX%"
if errorlevel 1 (
    rmdir /s /q "%CTX%"
    exit /b 1
)

rmdir /s /q "%CTX%"
echo ==> image pluto73-cross:latest ready; build the app with cross-compile\make.bat app
