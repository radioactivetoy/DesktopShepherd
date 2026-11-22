# Builds DesktopShepherd and writes the installer: dist\DesktopShepherd-Setup.exe.
# The installer is the program itself under a "Setup" name: double-click it to install
# (per user, no admin rights), run it again later to update. Remove it from
# Settings > Apps > Installed apps.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
cargo build --profile dist
if ($LASTEXITCODE -ne 0) { throw "build failed" }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\dist\desktopshepherd.exe dist\DesktopShepherd-Setup.exe -Force
Get-Item dist\DesktopShepherd-Setup.exe | ForEach-Object { "{0}  {1:N1} MB" -f $_.FullName, ($_.Length / 1MB) }
