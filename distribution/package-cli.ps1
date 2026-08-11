param(
    [Parameter(Mandatory = $true)][string]$TargetTriple,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoManifest = Get-Content -LiteralPath (Join-Path $repositoryRoot 'Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoManifest, '(?m)^version = "([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'workspace version was not found'
}
$version = $versionMatch.Groups[1].Value
$binaryDirectory = Join-Path $repositoryRoot "target/$TargetTriple/release"
$sonicmux = Join-Path $binaryDirectory 'sonicmux.exe'
$sonicmuxTui = Join-Path $binaryDirectory 'sonicmux-tui.exe'
if (-not (Test-Path -LiteralPath $sonicmux -PathType Leaf) -or
    -not (Test-Path -LiteralPath $sonicmuxTui -PathType Leaf)) {
    throw "release binaries were not found in $binaryDirectory"
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputDirectoryResolved = (Resolve-Path $OutputDirectory).Path
$archive = Join-Path $outputDirectoryResolved "sonicmux-v$version-$TargetTriple.zip"
if (Test-Path -LiteralPath $archive) {
    throw "refusing to replace existing archive: $archive"
}

$stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("sonicmux-package-" + [guid]::NewGuid())
try {
    $packageDirectory = Join-Path $stagingRoot "sonicmux-v$version-$TargetTriple"
    $completions = Join-Path $packageDirectory 'completions'
    New-Item -ItemType Directory -Path $completions -Force | Out-Null

    Copy-Item -LiteralPath $sonicmux, $sonicmuxTui -Destination $packageDirectory
    Copy-Item -LiteralPath @(
        (Join-Path $repositoryRoot 'README.md'),
        (Join-Path $repositoryRoot 'CHANGELOG.md'),
        (Join-Path $repositoryRoot 'LICENSE-APACHE'),
        (Join-Path $repositoryRoot 'LICENSE-MIT')
    ) -Destination $packageDirectory

    foreach ($shellName in @('bash', 'fish', 'powershell', 'zsh')) {
        & $sonicmux completions $shellName |
            Set-Content -LiteralPath (Join-Path $completions "sonicmux.$shellName") -Encoding utf8
        if ($LASTEXITCODE -ne 0) { throw "completion generation failed for $shellName" }
    }
    & $sonicmux man --output (Join-Path $packageDirectory 'sonicmux.1')
    if ($LASTEXITCODE -ne 0) { throw 'manual generation failed' }

    Compress-Archive -LiteralPath $packageDirectory -DestinationPath $archive
    Write-Output $archive
} finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }
}
