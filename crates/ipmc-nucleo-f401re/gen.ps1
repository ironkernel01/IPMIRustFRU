# gen.ps1 - build, flash and console helper for ipmc-nucleo-f401re on the NUCLEO-F401RE.
#   .\gen.ps1 build                  cargo build --release, .bin, section sizes
#   .\gen.ps1 flash                  build, then copy the .bin to the NOD_F401RE volume
#   .\gen.ps1 console <cmd> [...]    open COM3, send each argument as a command, print the reply
#   -Eval                            with build or flash: the evaluation image (IPMC_EVAL=1): the IPMB
#                                    stops after IPMC_EVAL_MINUTES (ipmi_defs_tier23.h). Needs the
#                                    Tier23 folder. The image is ipmc-nucleo-f401re-eval.bin (and
#                                    .elf) beside the normal one, so it is never mistaken for a full
#                                    image. Without -Eval the cargo call gets IPMC_EVAL=0, so a
#                                    build after an evaluation build carries no evaluation code.
#   -VolumeLabel <pattern>           flash target volume label (default NOD_F401RE)
#   Tier-2/3 extension: build sets IPMC_TIER23 for cargo (unless already set) to 1 when
#   ..\..\..\IPMIRustFRU_Tier23\ipmi_defs_tier23.h exists and to 0 otherwise, so a folder that
#   appears or disappears after an earlier build takes effect (cargo reruns the ipmc-defs build
#   script when the variable changes). Set IPMC_TIER23 yourself to override.
# Every failure exits non-zero with a message. "Flash OK" prints only after the copy, the drive
# check and the FAIL.TXT check all pass.
param(
    [Parameter(Position = 0, Mandatory = $true)][ValidateSet('build', 'flash', 'console')][string]$Action,
    [string]$VolumeLabel = 'NOD_F401RE',
    [switch]$Eval,
    [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Commands
)

# Native tools (cargo, objcopy) write progress to stderr, so the global preference stays
# 'Continue' and their failures are caught by explicit $LASTEXITCODE checks. Every cmdlet that
# can fail carries -ErrorAction Stop so its failure is terminating.
$ErrorActionPreference = 'Continue'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$root = $PSScriptRoot
$elf = Join-Path $root 'target\thumbv7em-none-eabihf\release\ipmc-nucleo-f401re'
$tier23Header = Join-Path $root '..\..\..\IPMIRustFRU_Tier23\ipmi_defs_tier23.h'
$bin = "$elf.bin"
if ($Eval) { $bin = "$elf-eval.bin" }
$ComPort = 'COM3'
# The baud rate is the IPMC_CONSOLE_BAUD line of ipmi_defs.h, the same define the firmware reads.
$DefsHeader = Join-Path $root '..\..\ipmi_defs.h'
$BaudLine = Select-String -Path $DefsHeader -Pattern '^#define\s+IPMC_CONSOLE_BAUD\s+(\d+)' -ErrorAction Stop | Select-Object -First 1
if (-not $BaudLine) { throw "IPMC_CONSOLE_BAUD not found in $DefsHeader" }
$Baud = [int]$BaudLine.Matches[0].Groups[1].Value
$CommandSettleMs = 400          # wait after sending a command before reading the reply
$CommandDeadlineMs = 3000       # overall time allowed to collect one command's reply
$ReplyBytesMax = 8192           # byte cap per command reply (the longest reply is about 700 bytes)
$FlashSettleSec = 3             # the debug interface programs the image after the copy
$RemountWaitSec = 10            # the volume may drop and return while programming

function Assert-Tool([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) { throw "required tool not found on PATH: $Name" }
}

function Invoke-Build {
    Assert-Tool 'cargo'
    Assert-Tool 'arm-none-eabi-objcopy'
    Assert-Tool 'arm-none-eabi-size'
    Push-Location $root
    $savedTier23 = $env:IPMC_TIER23
    $savedEval = $env:IPMC_EVAL
    try {
        if ($Eval) {
            if (-not (Test-Path $tier23Header -PathType Leaf)) { throw "-Eval needs the Tier23 folder: $tier23Header not found" }
            if ($env:IPMC_TIER23 -eq '0') { throw '-Eval needs the Tier-2/3 extension, but IPMC_TIER23 is 0' }
            $env:IPMC_EVAL = '1'
        } else {
            $env:IPMC_EVAL = '0'
        }
        if (-not $env:IPMC_TIER23) {
            if (Test-Path $tier23Header -PathType Leaf) {
                $env:IPMC_TIER23 = '1'
                Write-Host "Tier-2/3 extension: built from $((Resolve-Path (Split-Path $tier23Header)).Path)"
            } else {
                $env:IPMC_TIER23 = '0'
                Write-Host 'Tier-2/3 extension: not present, Tier-1 build'
            }
        }
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
        if ($Eval) { Copy-Item $elf "$elf-eval.elf" -Force -ErrorAction Stop }
        arm-none-eabi-objcopy -O binary $elf $bin
        if ($LASTEXITCODE -ne 0) { throw "objcopy failed (exit $LASTEXITCODE)" }
        arm-none-eabi-size $elf
        if ($LASTEXITCODE -ne 0) { throw "size failed (exit $LASTEXITCODE)" }
        $len = (Get-Item $bin -ErrorAction Stop).Length
        if ($len -le 0) { throw "image is empty: $bin" }
        Write-Host "Image: $bin ($len bytes)"
        if ($Eval) {
            $minutes = (Select-String -Path $tier23Header -Pattern '^#define\s+IPMC_EVAL_MINUTES\s+(\d+)' -ErrorAction Stop | Select-Object -First 1).Matches[0].Groups[1].Value
            Write-Host "EVALUATION image: $bin (IPMB stops after $minutes minutes)"
        }
    } finally {
        $env:IPMC_TIER23 = $savedTier23
        $env:IPMC_EVAL = $savedEval
        Pop-Location
    }
}

function Invoke-Flash {
    Invoke-Build
    $vol = Get-Volume -ErrorAction Stop | Where-Object FileSystemLabel -like $VolumeLabel | Select-Object -First 1
    if (-not $vol -or -not $vol.DriveLetter) { throw "$VolumeLabel volume not found" }
    $drive = "$($vol.DriveLetter):\"
    Write-Host "Flashing to $drive"
    Copy-Item $bin $drive -Force -ErrorAction Stop
    Start-Sleep -Seconds $FlashSettleSec
    $deadline = (Get-Date).AddSeconds($RemountWaitSec)
    while (-not (Test-Path $drive -ErrorAction Stop)) {
        if ((Get-Date) -gt $deadline) { throw "drive $drive did not return after the copy" }
        Start-Sleep -Milliseconds 250
    }
    $failFile = Join-Path $drive 'FAIL.TXT'
    if (Test-Path $failFile -ErrorAction Stop) {
        Get-Content $failFile -ErrorAction Stop
        throw 'FAIL.TXT present: flash failed'
    }
    Write-Host 'Flash OK (copy done, drive present, no FAIL.TXT)'
}

function Invoke-Console {
    $port = New-Object System.IO.Ports.SerialPort $ComPort, $Baud, 'None', 8, 'One'
    $port.ReadTimeout = 500
    $port.DtrEnable = $true
    $port.Open()
    try {
        $port.DiscardInBuffer()
        foreach ($cmd in $Commands) {
            $port.Write("$cmd`r")
            Start-Sleep -Milliseconds $CommandSettleMs
            $out = New-Object System.Text.StringBuilder
            $clock = [System.Diagnostics.Stopwatch]::StartNew()
            try {
                while ($clock.ElapsedMilliseconds -lt $CommandDeadlineMs -and $out.Length -lt $ReplyBytesMax) {
                    [void]$out.Append([char]$port.ReadChar())
                }
            } catch [System.TimeoutException] {}
            if ($clock.ElapsedMilliseconds -ge $CommandDeadlineMs -or $out.Length -ge $ReplyBytesMax) {
                Write-Warning "reply to '$cmd' cut off (deadline $CommandDeadlineMs ms or $ReplyBytesMax bytes)"
            }
            $out.ToString()
        }
    } finally {
        $port.Close()
    }
}

try {
    switch ($Action) {
        'build'   { Invoke-Build }
        'flash'   { Invoke-Flash }
        'console' { Invoke-Console }
    }
} catch {
    Write-Host "gen.ps1: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
