<#
.SYNOPSIS
  Assembles a portable imagePicker zip (Windows) from an existing app folder. Builds nothing.

.DESCRIPTION
  Portable mode = an empty file `imagepicker.portable` next to the executable; the app then keeps
  its catalog, caches, settings, logs, models and AI runtime in `data\` beside it
  (docs/02 section 7, docs/release.md "Portable zip").

  -AppDir must contain imagePicker.exe, and normally also uv.exe, web\ and ai-worker\ (the bundled
  resources); missing pieces are reported as warnings. Typical sources: the folder of an installed
  NSIS build (%LOCALAPPDATA%\imagePicker) or target\release of a local `tauri build --no-bundle`.

.EXAMPLE
  pwsh scripts/make-portable.ps1 -AppDir "$env:LOCALAPPDATA\imagePicker" -Version 0.2.0
#>
param(
  [Parameter(Mandatory = $true)][string]$AppDir,
  [string]$Version = "dev",
  [string]$OutDir = (Get-Location).Path,
  [string]$ExeName = "imagePicker.exe"
)
$ErrorActionPreference = "Stop"

$src = (Resolve-Path -LiteralPath $AppDir).Path
if (-not (Test-Path -LiteralPath (Join-Path $src $ExeName))) {
  throw "$ExeName not found in $src"
}
foreach ($need in @("uv.exe", "web\index.html", "ai-worker\pyproject.toml")) {
  if (-not (Test-Path -LiteralPath (Join-Path $src $need))) {
    Write-Warning "$need is missing in $src; the AI components cannot be installed without it"
  }
}

$name = "imagePicker_${Version}_x64-portable"
$stage = Join-Path ([IO.Path]::GetTempPath()) ("ip-portable-" + [Guid]::NewGuid().ToString("N"))
$root = Join-Path $stage "imagePicker"
New-Item -ItemType Directory -Path $root | Out-Null
try {
  # copy the app folder; leave out uninstallers, debug symbols and any data of the source install
  $skip = @("data", "imagepicker.portable")
  Get-ChildItem -LiteralPath $src -Force | Where-Object {
    $skip -notcontains $_.Name -and $_.Name -notlike "uninstall*" -and $_.Name -notlike "unins*" -and $_.Extension -ne ".pdb"
  } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $root -Recurse -Force }

  # the marker that switches portable mode on
  New-Item -ItemType File -Path (Join-Path $root "imagepicker.portable") | Out-Null
  $readme = @(
    "imagePicker portable",
    "",
    "Start imagePicker.exe. All data (catalog, caches, settings, logs, models, AI components) is kept",
    "in the data folder next to it; delete the file imagepicker.portable to use the normal profile folder.",
    "If the drive gets another letter, photo folders on this same drive are followed automatically.",
    "Order: --data-dir, then IMAGEPICKER_DATA_DIR, then this portable folder, then %APPDATA%\imagePicker."
  )
  [IO.File]::WriteAllLines((Join-Path $root "PORTABLE.txt"), $readme, (New-Object Text.UTF8Encoding $false))

  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
  $zip = Join-Path (Resolve-Path -LiteralPath $OutDir).Path "$name.zip"
  if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
  Compress-Archive -Path $root -DestinationPath $zip -CompressionLevel Optimal
  $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
  [IO.File]::WriteAllText("$zip.sha256", "$hash  $name.zip`n", (New-Object Text.UTF8Encoding $false))
  Write-Host "wrote $zip"
  Write-Host "sha256 $hash"
}
finally {
  Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
