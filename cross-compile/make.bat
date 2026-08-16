@echo off
:: Run a pluto73 Makefile target inside the cross-build container.
::
::   cross-compile\make.bat app               # cross-compile the app for the Pluto
::   cross-compile\make.bat app-diagnostics   # ditto, diagnostics binary
::
setlocal enabledelayedexpansion

set HERE=%~dp0
for %%I in ("%HERE%..") do set APP_DIR=%%~fI

set TARGET=%*
if "%TARGET%"=="" set TARGET=app

docker run --rm --platform linux/amd64 ^
  -v "%APP_DIR%:/work" ^
  -v pluto73-cargo-registry:/usr/local/cargo/registry ^
  -w /work ^
  pluto73-cross:latest ^
  make %TARGET% ^
    CROSS_COMPILE_PATH=/opt/toolchain/bin ^
    TOOLCHAIN_DIR=/opt/toolchain ^
    SYSROOT=/opt/toolchain/arm-linux-gnueabihf/libc
