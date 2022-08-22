$ErrorActionPreference = "Stop"
cargo ndk -t arm64-v8a -o app/src/main/jniLibs/ -- build
if ($lastexitcode -ne 0) {
    throw "Error"
}

./gradlew.bat buildDebug
if ($lastexitcode -ne 0) {
    throw "Error"
}

./gradlew.bat installDebug
if ($lastexitcode -ne 0) {
    throw "Error"
}
