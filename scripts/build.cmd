@echo off
setlocal
pushd "%~dp0.."
cargo build --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked
if errorlevel 1 (popd & exit /b 1)
start "" "%CD%\src\target\debug\mixerplus.exe"
popd
