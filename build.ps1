$ErrorActionPreference = "Stop"
$env:RUSTFLAGS = "-C target-feature=+crt-static"

cargo +nightly-x86_64-pc-windows-msvc clean
if ($lastexitcode -ne 0) {
    throw "Error"
}

cargo +nightly-x86_64-pc-windows-msvc build --release
if ($lastexitcode -ne 0) {
    throw "Error"
}

Copy-Item "target\release\crusader.exe" -Destination "crusader-v0-x86_64.exe"
Copy-Item "target\release\crusader-gui.exe" -Destination "crusader-gui-v0-x86_64.exe"

cargo +nightly-i686-pc-windows-msvc clean
if ($lastexitcode -ne 0) {
    throw "Error"
}

cargo +nightly-i686-pc-windows-msvc build --release
if ($lastexitcode -ne 0) {
    throw "Error"
}

Copy-Item "target\release\crusader.exe" -Destination "crusader-v0-i686.exe"
Copy-Item "target\release\crusader-gui.exe" -Destination "crusader-gui-v0-i686.exe"

cargo +nightly-i686-pc-windows-msvc clean
if ($lastexitcode -ne 0) {
    throw "Error"
}
