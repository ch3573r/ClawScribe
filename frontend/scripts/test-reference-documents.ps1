$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ([string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER) -or
    $env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER -or
    $env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER) {
    throw 'Document tests require the designated local runner.'
}
$sourceRoot = (Resolve-Path (Join-Path $PSScriptRoot '../src-tauri/src/knowledge/documents')).Path.Replace('\', '/')
$testRoot = Join-Path $env:RUNNER_TEMP "clawscribe-document-tests-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT"
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../src-tauri/migrations') -Destination $testRoot -Recurse -Force
$storeModule = ''
if (Test-Path -LiteralPath (Join-Path $sourceRoot 'store.rs')) {
    $storeModule = "#[path = `"$sourceRoot/store.rs`"] pub mod store;"
}
$importModule = ''
if (Test-Path -LiteralPath (Join-Path $sourceRoot 'import.rs')) {
    $importModule = "#[path = `"$sourceRoot/import.rs`"] pub mod import;"
}
$workerModule = ''
$workerBinary = ''
if (Test-Path -LiteralPath (Join-Path $sourceRoot 'worker.rs')) {
    $workerModule = "#[path = `"$sourceRoot/worker.rs`"] pub mod worker;"
    $workerBinary = @'
[[bin]]
name = "reference-worker"
path = "main.rs"
'@
    @'
fn main() {
    match clawscribe_document_tests::documents::worker::dispatch() {
        Some(code) => std::process::exit(code),
        None => std::process::exit(2),
    }
}
'@ | Set-Content -LiteralPath (Join-Path $testRoot 'main.rs') -Encoding utf8NoBOM
}
@"
pub mod documents {
    #[path = "$sourceRoot/types.rs"] mod types;
    pub use types::*;
    #[path = "$sourceRoot/extract.rs"] pub mod extract;
    $workerModule
    $storeModule
    $importModule
    #[cfg(test)] #[path = "$sourceRoot/fixtures.rs"] pub mod fixtures;
    #[cfg(test)] #[path = "$sourceRoot/tests.rs"] mod tests;
}
"@ | Set-Content -LiteralPath (Join-Path $testRoot 'lib.rs') -Encoding utf8NoBOM
@"
[package]
name = "clawscribe-document-tests"
version = "0.0.0"
edition = "2021"
[workspace]
[lib]
path = "lib.rs"
[dependencies]
lopdf = { version = "=0.34.0", default-features = false, features = ["nom_parser"] }
quick-xml = "=0.37.5"
zip = "=2.4.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"
tempfile = "3"
windows = { version = "0.57", features = ["Win32_Foundation", "Win32_Security", "Win32_System_JobObjects", "Win32_System_Threading"] }
sqlx = { version = "=0.8.6", default-features = false, features = ["runtime-tokio", "sqlite", "macros", "migrate"] }
uuid = { version = "1", features = ["v4"] }
sha2 = "0.10"
$workerBinary
"@ | Set-Content -LiteralPath (Join-Path $testRoot 'Cargo.toml') -Encoding utf8NoBOM
# Shared production parser/worker modules, without the unrelated desktop/ML link.
rustc --version
if ($workerBinary) {
    cargo build --manifest-path (Join-Path $testRoot 'Cargo.toml') --bin reference-worker
    $env:CLAWSCRIBE_DOCUMENT_TEST_WORKER = Join-Path $testRoot 'target/debug/reference-worker.exe'
}
cargo test --manifest-path (Join-Path $testRoot 'Cargo.toml') --lib -- --test-threads=1
