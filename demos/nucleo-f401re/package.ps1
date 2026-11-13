# Build and package the public Tier-1 demo; never flashes a board.
param()
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$savedPath = $env:PATH
$savedTier = $env:IPMC_TIER23
$savedEval = $env:IPMC_EVAL
$savedField = $env:IPMC_FIELD
$savedTarget = $env:CARGO_TARGET_DIR

try {
    $env:PATH = "$cargoBin;$savedPath"
    foreach ($tool in @('cargo', 'rustc', 'arm-none-eabi-objcopy', 'arm-none-eabi-size', 'python', 'git')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "required tool not found: $tool; no demo images were packaged"
        }
    }
    $env:IPMC_TIER23 = '0'
    $env:IPMC_EVAL = '0'
    $env:IPMC_FIELD = '0'
    # gen.ps1 expects the firmware crate's own target directory.
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    Push-Location $repo
    try {
        $revision = (& git rev-parse HEAD)
        if ($LASTEXITCODE -ne 0) { throw 'cannot read source revision' }
        $dirty = [bool](& git status --porcelain --untracked-files=normal)
        if ($LASTEXITCODE -ne 0) { throw 'cannot read source status' }
        $compiler = (& rustc --version)
        if ($LASTEXITCODE -ne 0) { throw 'cannot read the pinned Rust compiler version' }
        $started = [DateTime]::UtcNow
        # Use a child process: gen.ps1 uses exit on failure.
        & powershell -NoProfile -ExecutionPolicy Bypass -File crates/ipmc-nucleo-f401re/gen.ps1 build
        if ($LASTEXITCODE -ne 0) { throw 'Tier-1 build failed; no demo images were packaged' }

        $release = Join-Path $repo 'crates\ipmc-nucleo-f401re\target\thumbv7em-none-eabihf\release'
        $board = Join-Path $release 'ipmc-board.bin'
        $hex = Join-Path $release 'ipmc-nucleo-f401re.hex'
        foreach ($file in @($board, $hex)) {
            $item = Get-Item -LiteralPath $file
            if ($item.Length -eq 0 -or $item.LastWriteTimeUtc -lt $started) {
                throw "image is empty or predates this build: $file"
            }
        }
        # The build helper already checks the runtime header and SHA-256 digest.
        $identity = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($board))
        if (-not $identity.Contains('VPX Tier-1 IPMC') -or
            -not $identity.Contains('Tier-1 IPMC Reference') -or
            $identity -match 'VPX Tier-[23] IPMC|Tier-[23] IPMC Reference') {
            throw 'board image is not the expected Tier-1 demo; no images were packaged'
        }

        $images = Join-Path $PSScriptRoot 'images'
        New-Item -ItemType Directory -Force -Path $images | Out-Null
        $entries = @()
        foreach ($pair in @(
            @($board, 'ipmc-board-tier1.bin'),
            @($hex, 'ipmc-nucleo-f401re-tier1.hex')
        )) {
            $destination = Join-Path $images $pair[1]
            Copy-Item -LiteralPath $pair[0] -Destination $destination
            $entries += [ordered]@{
                file = $pair[1]
                bytes = (Get-Item -LiteralPath $destination).Length
                sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
        $manifest = [ordered]@{
            board = 'NUCLEO-F401RE'
            tier = 1
            built_utc = [DateTime]::UtcNow.ToString('o')
            source_revision = "$revision".Trim()
            source_dirty = $dirty
            rustc = "$compiler".Trim()
            configuration_sha256 = (Get-FileHash -LiteralPath (Join-Path $repo 'crates\ipmc-defs\src\defs.rs') -Algorithm SHA256).Hash.ToLowerInvariant()
            tier23_override = 0
            evaluation = $false
            field = $false
            board_tested = $false
            license = '../../../LICENSE.md'
            images = $entries
        }
        $utf8 = New-Object Text.UTF8Encoding $false
        [IO.File]::WriteAllText((Join-Path $images 'manifest.json'), ($manifest | ConvertTo-Json -Depth 5) + "`n", $utf8)
        $sums = ($entries | ForEach-Object { "$($_.sha256)  $($_.file)" }) -join "`n"
        [IO.File]::WriteAllText((Join-Path $images 'SHA256SUMS.txt'), $sums + "`n", $utf8)
        Write-Host "Packaged Tier-1 demo images in $images; board testing has not been performed by this script."
    } finally {
        Pop-Location
    }
} finally {
    $env:PATH = $savedPath
    $env:IPMC_TIER23 = $savedTier
    $env:IPMC_EVAL = $savedEval
    $env:IPMC_FIELD = $savedField
    $env:CARGO_TARGET_DIR = $savedTarget
}
