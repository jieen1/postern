# Build the Postern Console desktop app (.msi/.exe) ON Windows.
# Needs: Rust+cargo, Node + pnpm, WebView2 runtime (Win10+ has it).
# Run from the repo root in PowerShell:  .\dist-windows\build-console.ps1
$ErrorActionPreference = "Stop"
Push-Location "$PSScriptRoot\..\web"
pnpm install
pnpm tauri build
Write-Host "Console built at web\src-tauri\target\release\bundle\ (msi / nsis)."
Pop-Location
