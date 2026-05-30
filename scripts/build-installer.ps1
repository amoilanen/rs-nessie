# Build platform-native installers locally using the Tauri bundler (Windows host).
#
# Invokes `npm --prefix app run tauri build` with `--bundles nsis` to produce
# an NSIS installer. Fails fast on missing toolchains (node, npm, cargo, rustc).

$ErrorActionPreference = 'Stop'

function Write-Log {
    param([string]$Message)
    Write-Host "[build-installer] $Message"
}

function Test-Cmd {
    param([string]$Cmd)
    if (-not (Get-Command $Cmd -ErrorAction SilentlyContinue)) {
        Write-Error "missing required tool: $Cmd"
        exit 1
    }
}

Test-Cmd node
Test-Cmd npm
Test-Cmd cargo
Test-Cmd rustc

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Resolve-Path (Join-Path $ScriptDir '..')

Push-Location $RepoRoot
try {
    if (-not (Test-Path (Join-Path $RepoRoot 'app/node_modules'))) {
        Write-Log "installing frontend dependencies (npm ci)"
        npm --prefix app ci
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    Write-Log "building Tauri bundles: nsis"
    npm --prefix app run tauri build -- --bundles nsis
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Log "produced artifacts:"
    $patterns = @(
        (Join-Path $RepoRoot 'app/src-tauri/target/release/bundle/nsis/*.exe'),
        (Join-Path $RepoRoot 'target/release/bundle/nsis/*.exe')
    )
    $found = $false
    foreach ($p in $patterns) {
        Get-ChildItem -Path $p -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "  $($_.FullName)"
            $found = $true
        }
    }

    if (-not $found) {
        Write-Error "no installer artifacts found after build"
        exit 1
    }

    Write-Log "done."
}
finally {
    Pop-Location
}
