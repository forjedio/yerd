$ErrorActionPreference = "Stop"

$gui = Split-Path -Parent $PSScriptRoot
$workspace = Resolve-Path (Join-Path $gui "..\..")
$target = "x86_64-pc-windows-msvc"
$binaries = Join-Path $gui "src-tauri\binaries"
$cargo = (Get-Command cargo -ErrorAction Stop).Source
$cargoBin = Split-Path -Parent $cargo
$managedBin = Join-Path $env:APPDATA "yerd\Yerd\data\bin"
$npm = Get-Command npm.cmd -ErrorAction SilentlyContinue
if ($npm) {
    $npm = $npm.Source
    $nodeBin = Split-Path -Parent (Get-Command node -ErrorAction Stop).Source
} else {
    $npm = Join-Path $managedBin "npm.cmd"
    $nodeBin = $managedBin
}
$env:PATH = "$cargoBin;$nodeBin;$env:SystemRoot\System32;$env:SystemRoot"

& $cargo build --release --target $target -p yerd -p yerdd -p yerd-helper
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path $binaries | Out-Null
foreach ($name in @("yerd", "yerdd", "yerd-helper")) {
    $source = Join-Path $workspace "target\$target\release\$name.exe"
    $destination = Join-Path $binaries "$name-$target.exe"
    Copy-Item -Force -LiteralPath $source -Destination $destination
}

Push-Location $gui
try {
    & $npm run tauri build -- --config src-tauri/tauri.bundle-windows.conf.json
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
