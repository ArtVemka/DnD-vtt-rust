# Builds a portable folder and zip: exe + renderer + portable.txt next to it.
# Campaigns then live in .\data and nothing touches %APPDATA%.
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

if (-not $SkipBuild) {
  Push-Location $root
  $previous = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  cargo build -p dnd-desktop --release
  $code = $LASTEXITCODE
  $ErrorActionPreference = $previous
  Pop-Location
  if ($code -ne 0) { throw "cargo build failed with exit code $code" }
}

$exe = Join-Path $root 'target\release\dnd-desktop.exe'
if (-not (Test-Path $exe)) { throw "release exe not found: $exe" }

$out = Join-Path $root 'dist\dnd-table-portable'
Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $out -Force | Out-Null

Copy-Item $exe (Join-Path $out 'DnD Table.exe')
Copy-Item (Join-Path $root 'renderer') (Join-Path $out 'renderer') -Recurse
Set-Content -Path (Join-Path $out 'portable.txt') -Value 'portable' -Encoding UTF8
Set-Content -Path (Join-Path $out 'README.txt') -Encoding UTF8 -Value @'
Portable build.
Run "DnD Table.exe". Campaigns, backups and uploaded files live in the data
folder next to the program. Nothing is written to the Windows profile.
'@

$zip = Join-Path $root 'dist\DnD-Table-portable.zip'
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zip -CompressionLevel Optimal

$size = [math]::Round((Get-Item $zip).Length / 1MB, 2)
Write-Host "Portable folder: $out"
Write-Host "Portable zip:    $zip ($size MB)"
