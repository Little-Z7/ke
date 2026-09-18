# Added by ke: Windows installer for ke (壳).
#
#   powershell -ExecutionPolicy Bypass -c "irm https://github.com/Little-Z7/ke/releases/latest/download/ke-install.ps1 | iex"
#
# Environment:
#   KE_INSTALL_DIR    install directory (default: %LOCALAPPDATA%\Programs\ke)
#   KE_RELEASE_TAG    install this release tag (e.g. ke-v0.2.0) instead of latest
#   KE_DOWNLOAD_BASE  download assets from this base URL instead of GitHub

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Repo = "Little-Z7/ke"
$Asset = "ke-windows-x86_64.zip"
$InstallDir = if (-not [string]::IsNullOrWhiteSpace($env:KE_INSTALL_DIR)) {
    $env:KE_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "Programs\ke"
}

function Write-Step([string]$Message) {
    Write-Host "  > $Message"
}

function Fail([string]$Message) {
    Write-Host "  x $Message" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "  ke (壳) installer — https://github.com/$Repo"
Write-Host ""

if (-not [string]::IsNullOrWhiteSpace($env:KE_DOWNLOAD_BASE)) {
    $Base = $env:KE_DOWNLOAD_BASE.TrimEnd("/")
} elseif (-not [string]::IsNullOrWhiteSpace($env:KE_RELEASE_TAG)) {
    $Base = "https://github.com/$Repo/releases/download/$($env:KE_RELEASE_TAG)"
} else {
    $Base = "https://github.com/$Repo/releases/latest/download"
}

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ke-install-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    Write-Step "fetching checksums..."
    $sumsPath = Join-Path $Tmp "SHA256SUMS"
    try {
        Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing
    } catch {
        Fail "can't fetch $Base/SHA256SUMS"
    }

    $expected = $null
    foreach ($line in Get-Content -LiteralPath $sumsPath) {
        $parts = $line.Trim() -split "\s+"
        if ($parts.Count -ge 2 -and ($parts[1] -eq $Asset -or $parts[1] -eq "*$Asset")) {
            $expected = $parts[0].ToLowerInvariant()
            break
        }
    }
    if ([string]::IsNullOrWhiteSpace($expected) -or $expected.Length -ne 64) {
        Fail "this release has no binary for windows/x86_64"
    }

    Write-Step "downloading $Asset..."
    $zipPath = Join-Path $Tmp $Asset
    try {
        Invoke-WebRequest -Uri "$Base/$Asset" -OutFile $zipPath -UseBasicParsing
    } catch {
        Fail "download failed from $Base/$Asset"
    }

    $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "downloaded ke checksum did not match"
    }

    $extract = Join-Path $Tmp "extract"
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extract -Force
    $payload = Get-ChildItem -LiteralPath $extract -Filter "herdr.exe" -Recurse | Select-Object -First 1
    if ($null -eq $payload) {
        Fail "zip did not contain herdr.exe"
    }
    $conpty = Join-Path $payload.DirectoryName "conpty"
    if (-not (Test-Path -LiteralPath $conpty)) {
        Fail "zip did not contain the ConPTY runtime"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $staged = Join-Path $InstallDir ".ke.staging.exe"
    Copy-Item -LiteralPath $payload.FullName -Destination $staged -Force
    Move-Item -LiteralPath $staged -Destination (Join-Path $InstallDir "ke.exe") -Force
    $destConpty = Join-Path $InstallDir "conpty"
    if (Test-Path -LiteralPath $destConpty) {
        Remove-Item -LiteralPath $destConpty -Recurse -Force
    }
    Copy-Item -LiteralPath $conpty -Destination $destConpty -Recurse -Force

    $exe = Join-Path $InstallDir "ke.exe"
    $version = & $exe --version 2>$null
    Write-Step "installed $exe$(if ($version) { " — $version" })"

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @()
    if (-not [string]::IsNullOrWhiteSpace($userPath)) {
        $entries = $userPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }
    if ($entries -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable("Path", ($entries + $InstallDir) -join ";", "User")
        $env:Path = "$InstallDir;$env:Path"
        Write-Step "added $InstallDir to your user PATH (open a new terminal if ke is not found)"
    }

    Write-Host ""
    Write-Step "run 'ke' to start. ke keeps its own config under %APPDATA%\ke and does not touch an installed herdr."
    Write-Step "to update, run this installer again."
    Write-Host ""
} finally {
    Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
