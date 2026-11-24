<#
 Makes the three winget manifest files for one published release.
 Usage:  .\make-manifest.ps1 -Tag v1.1.0
 It downloads that release's SHA256SUMS.txt (read-only), takes the Setup file's SHA-256, and writes
 the three YAML files with the new version, URL and hash to packaging\winget\out\<version>\.
 The release must already be published and must contain the --silent switch.
#>
param(
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$Repo = 'radioactivetoy/DesktopShepherd',
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'
if ($Tag -notmatch '^v\d+\.\d+\.\d+([-.][0-9A-Za-z.]+)?$') { throw "Tag '$Tag' should look like v1.2.3" }
$version = $Tag.Substring(1)
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $OutDir) { $OutDir = Join-Path $here "out\$version" }
$setup = "DesktopShepherd-Setup-$version.exe"
$base = "https://github.com/$Repo/releases/download/$Tag"

$sums = (Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS.txt").Content
if ($sums -is [byte[]]) { $sums = [Text.Encoding]::ASCII.GetString($sums) }
$line = ($sums -split "`r?`n") | Where-Object { $_ -match [regex]::Escape($setup) + '\s*$' } | Select-Object -First 1
if (-not $line) { throw "$setup not found in SHA256SUMS.txt" }
$hash = ($line -split '\s+')[0].ToUpper()
if ($hash -notmatch '^[0-9A-F]{64}$') { throw "Bad hash: $hash" }

New-Item -ItemType Directory -Force $OutDir | Out-Null
foreach ($f in 'radioactivetoy.DesktopShepherd.yaml', 'radioactivetoy.DesktopShepherd.installer.yaml', 'radioactivetoy.DesktopShepherd.locale.en-US.yaml') {
    $t = Get-Content (Join-Path $here $f) -Raw
    $t = $t -replace '(?m)^(PackageVersion: ).*$', "`${1}$version"
    $t = $t -replace '(?m)^(\s+DisplayVersion: ).*$', "`${1}$version"
    $t = $t -replace '(?m)^(\s+InstallerUrl: ).*$', "`${1}$base/$setup"
    $t = $t -replace '(?m)^(\s+InstallerSha256: ).*$', "`${1}$hash"
    $t = $t -replace '(?m)^(ReleaseNotesUrl: ).*$', "`${1}https://github.com/$Repo/releases/tag/$Tag"
    $t = $t -replace '(?m)^# NOTE: rc\.2 has no --silent.*\r?\n', ''
    [IO.File]::WriteAllText((Join-Path $OutDir $f), ($t -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding($false)))
}
Write-Host "Written to $OutDir (version $version, SHA-256 $hash)"
Write-Host "Next: winget validate --manifest `"$OutDir`""
