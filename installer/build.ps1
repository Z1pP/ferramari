<#
.SYNOPSIS
Build the Windows installer and portable archive for Ferramari.

.DESCRIPTION
Runs the release workspace build (unless -NoBuild is given), stages both
binaries with README/LICENSE and the compiled gettext catalogs, then invokes
Inno Setup 6. Outputs land in installer\Output:

  Ferramari-<version>-setup.exe
  Ferramari-<version>-portable.zip

Inno Setup 6 must be installed (ISCC.exe is looked up in the usual Program
Files locations). GNU gettext's msgfmt is optional; without it the installer
contains the source (English) strings and the script prints a warning.

.PARAMETER NoBuild
Reuse an existing target\release build instead of invoking cargo.

.PARAMETER Version
Override the version read from Cargo.toml.
#>
[CmdletBinding()]
param(
    [switch]$NoBuild,
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$stage = $null

function Find-InnoSetup {
    $candidates = @()
    if (${env:ProgramFiles(x86)}) {
        $candidates += Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
    }
    if ($env:ProgramFiles) {
        $candidates += Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"
    }
    if ($env:LOCALAPPDATA) {
        # winget installs Inno Setup per user by default.
        $candidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    $command = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    return $null
}

function Find-Msgfmt {
    $command = Get-Command msgfmt.exe -ErrorAction SilentlyContinue
    if (-not $command) {
        $command = Get-Command msgfmt -ErrorAction SilentlyContinue
    }
    if ($command) {
        return $command.Source
    }
    if ($env:LOCALAPPDATA) {
        # The mlocati gettext build installs here when winget is not elevated.
        $candidate = Join-Path $env:LOCALAPPDATA "Programs\gettext-iconv\bin\msgfmt.exe"
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    return $null
}

function Read-CargoVersion {
    $match = Select-String -Path (Join-Path $root "Cargo.toml") `
        -Pattern '^version = "(.+)"' | Select-Object -First 1
    if (-not $match) {
        throw "Could not read the workspace version from Cargo.toml"
    }
    return $match.Matches[0].Groups[1].Value
}

Push-Location $root
try {
    if (-not $Version) {
        $Version = Read-CargoVersion
    }
    Write-Host "Building Ferramari $Version"

    if (-not $NoBuild) {
        & cargo build --workspace --release --locked
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    }

    $release = Join-Path $root "target\release"
    foreach ($name in @("ferramari.exe", "ferramari-ui.exe")) {
        $path = Join-Path $release $name
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Missing $path; run cargo build --workspace --release --locked"
        }
    }

    $stage = Join-Path $env:TEMP ("ferramari-installer-" + [Guid]::NewGuid().ToString("N"))
    $bin = Join-Path $stage "bin"
    $locale = Join-Path $bin "locale"
    New-Item -ItemType Directory -Path $locale -Force | Out-Null

    Copy-Item -LiteralPath (Join-Path $release "ferramari.exe") -Destination $bin
    Copy-Item -LiteralPath (Join-Path $release "ferramari-ui.exe") -Destination $bin
    Copy-Item -LiteralPath (Join-Path $root "README.md") -Destination $bin
    Copy-Item -LiteralPath (Join-Path $root "LICENSE") -Destination $bin

    $msgfmt = Find-Msgfmt
    if ($msgfmt) {
        foreach ($po in Get-ChildItem -LiteralPath (Join-Path $root "po") -Filter "*.po") {
            $language = $po.BaseName
            $destination = Join-Path $locale "$language\LC_MESSAGES"
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            & $msgfmt --check -c -o (Join-Path $destination "ferramari.mo") $po.FullName
            if ($LASTEXITCODE -ne 0) {
                throw "msgfmt failed for $($po.Name)"
            }
        }
        Write-Host "Compiled locale catalogs from po\*.po"
    } else {
        Write-Warning "msgfmt not found: the package will contain English source strings only"
    }

    $output = Join-Path $root "installer\Output"
    New-Item -ItemType Directory -Path $output -Force | Out-Null

    $zip = Join-Path $output "Ferramari-$Version-portable.zip"
    if (Test-Path -LiteralPath $zip) {
        Remove-Item -LiteralPath $zip -Force
    }
    Compress-Archive -Path (Join-Path $bin "*") -DestinationPath $zip
    Write-Host "Wrote $zip"

    $iscc = Find-InnoSetup
    if (-not $iscc) {
        throw "Inno Setup 6 not found. Install it from https://jrsoftware.org/isinfo.php or run with -NoBuild after installing."
    }
    & $iscc (Join-Path $PSScriptRoot "ferramari.iss") `
        "/DAppVersion=$Version" `
        "/DSourceDir=$bin" `
        "/DLocaleDir=$locale" `
        "/DOutputDir=$output"
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE"
    }

    $setup = Join-Path $output "Ferramari-$Version-setup.exe"
    if (-not (Test-Path -LiteralPath $setup)) {
        throw "Inno Setup reported success but $setup is missing"
    }
    Write-Host "Wrote $setup"
} finally {
    Pop-Location
    if ($stage -and (Test-Path -LiteralPath $stage)) {
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
}
