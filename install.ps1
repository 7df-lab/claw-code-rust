# install.ps1 — Download and install the latest devo binary for Windows.
#
# Usage (run as administrator is not required, installs to user-local bin):
#   irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex
#
# Pin a specific version:
#   $env:VERSION = "v0.1.0"; irm https://raw.githubusercontent.com/7df-lab/devo/main/install.ps1 | iex

$ErrorActionPreference = "Stop"
$Repo = "7df-lab/devo"

# ── Platform detection ───────────────────────────────────────────────────
function Get-Target {
    $arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else {
        Write-Error "32-bit Windows is not supported"
        exit 1
    }
    return "${arch}-pc-windows-msvc"
}

# ── Resolve version ──────────────────────────────────────────────────────
function Resolve-Version {
    if ($env:VERSION) {
        return $env:VERSION
    }

    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    return $latest.tag_name
}

function Test-PathEntryPresent {
    param(
        [string]$PathValue,
        [string]$Entry
    )

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return $false
    }

    $normalizedEntry = [IO.Path]::TrimEndingDirectorySeparator($Entry)
    foreach ($candidate in ($PathValue -split ";")) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }

        $normalizedCandidate = [IO.Path]::TrimEndingDirectorySeparator($candidate.Trim())
        if ($normalizedCandidate -ieq $normalizedEntry) {
            return $true
        }
    }

    return $false
}

function Add-InstallDirToPath {
    param(
        [string]$InstallDir
    )

    $currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not (Test-PathEntryPresent -PathValue $currentUserPath -Entry $InstallDir)) {
        $newUserPath = if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
            $InstallDir
        } else {
            "$InstallDir;$currentUserPath"
        }
        [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    }

    if (-not (Test-PathEntryPresent -PathValue $env:Path -Entry $InstallDir)) {
        $env:Path = "$InstallDir;$env:Path"
    }
}

# ── Install ──────────────────────────────────────────────────────────────
function Main {
    $target = Get-Target
    $version = Resolve-Version
    $archiveUrl = "https://github.com/$Repo/releases/download/$version/devo-${version}-${target}.zip"

    Write-Host "Downloading devo $version for $target ..."

    $tmpDir = Join-Path $env:TEMP "devo-install"
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue | Out-Null
    New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

    try {
        $zipPath = Join-Path $tmpDir "devo.zip"
        Invoke-WebRequest -Uri $archiveUrl -OutFile $zipPath

        Expand-Archive -Path $zipPath -DestinationPath $tmpDir -Force

        # Locate devo.exe (it's inside a versioned subdirectory).
        $exe = Get-ChildItem -Recurse -Filter "devo.exe" -Path $tmpDir | Select-Object -First 1
        if (-not $exe) {
            Write-Error "devo.exe not found in the archive"
        }

        # Install target.
        $installDir = Join-Path $env:LOCALAPPDATA "Programs\devo"
        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        Copy-Item -Path $exe.FullName -Destination (Join-Path $installDir "devo.exe") -Force

        Add-InstallDirToPath -InstallDir $installDir

        Write-Host "Installed devo to ${installDir}\devo.exe"
        Write-Host "PATH was updated for future terminals."
        Write-Host "Open a new terminal, or run:"
        Write-Host "  `$env:Path = `"$installDir;`$env:Path`""
        Write-Host "Run 'devo onboard' to get started."
    }
    finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue | Out-Null
    }
}

Main
