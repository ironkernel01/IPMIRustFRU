# gen.ps1 - build, flash and console helper for ipmc-nucleo-f401re on the NUCLEO-F401RE.
#   .\gen.ps1 build                  cargo build --release, .bin, section sizes
#   .\gen.ps1 flash                  build, then program the board: over SWD with OpenOCD when it
#                                    can, else by copying the .bin to the NOD_F401RE volume
#   .\gen.ps1 flash -Drive           build, then copy the .bin to the NOD_F401RE volume (forced)
#   .\gen.ps1 flash -Swd             build, then program the ELF over SWD with OpenOCD (forced)
#   .\gen.ps1 console <cmd> [...]    open COM3, send each argument as a command, print the reply
#   -Eval                            with build or flash: the evaluation image (IPMC_EVAL=1): the IPMB
#                                    stops after IPMC_EVAL_MINUTES (defs_tier23.rs). Needs the
#                                    Tier23 folder. The image is ipmc-nucleo-f401re-eval.bin (and
#                                    .elf) beside the normal one, so it is never mistaken for a full
#                                    image. Without -Eval the cargo call gets IPMC_EVAL=0, so a
#                                    build after an evaluation build carries no evaluation code.
#   -VolumeLabel <pattern>           flash target volume label (default NOD_F401RE)
#   Tier-2/3 extension: build sets IPMC_TIER23 for cargo (unless already set) to 1 when
#   ..\..\..\IPMIRustFRU_Tier23\defs_tier23.rs exists and to 0 otherwise, so a folder that
#   appears or disappears after an earlier build takes effect (cargo reruns the ipmc-defs build
#   script when the variable changes). Set IPMC_TIER23 yourself to override.
# Flash paths. SWD runs: openocd -f interface/stlink.cfg -f target/stm32f4x.cfg
#   -c "program <release ELF> verify reset exit". OpenOCD is on PATH through winget (xPack), and
#   the ST-LINK debug interface needs the WinUSB driver (bound with Zadig, owner action). Plain
#   `flash` takes SWD when OpenOCD is installed and finds the probe, and says so; it falls back to
#   the drive copy only when OpenOCD is absent or reports no probe (it could not open the ST-LINK).
#   A SWD program that starts and fails is an error, never a silent fallback. -Swd never falls
#   back, -Drive never uses OpenOCD; both together are refused. The script prints which path it took.
#   Both paths program the image of the build this call just made (the .bin is rewritten by every
#   build; the ELF is the one it was made from), checked before anything is programmed.
# The .bin is made with objcopy --gap-fill 0xFF: the image spans the vector sector (sector 0), the
#   FRU store sector (sector 1, link.x and memory.x) and the program (sectors 2-7), and the gap
#   over the store must read FFh (erased), not 00h. A drive copy therefore leaves the FRU store
#   erased: a reflash by the drive erases the unit's FRU image and settings (the owner accepts
#   that). An SWD program of the ELF writes only the sections of the ELF, so the store sector
#   keeps what it holds (the unit's FRU image survives a reflash).
# Every failure exits non-zero with a message. "Flash OK" prints only after the programming, and
# for the drive copy the drive check and the FAIL.TXT check, all pass.
param(
    [Parameter(Position = 0, Mandatory = $true)][ValidateSet('build', 'flash', 'console')][string]$Action,
    [string]$VolumeLabel = 'NOD_F401RE',
    [switch]$Eval,
    [switch]$Drive,
    [switch]$Swd,
    [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Commands
)

# Native tools (cargo, objcopy) write progress to stderr, so the global preference stays
# 'Continue' and their failures are caught by explicit $LASTEXITCODE checks. Every cmdlet that
# can fail carries -ErrorAction Stop so its failure is terminating.
$ErrorActionPreference = 'Continue'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$root = $PSScriptRoot
$elf = Join-Path $root 'target\thumbv7em-none-eabihf\release\ipmc-nucleo-f401re'
$tier23Header = Join-Path $root '..\..\..\IPMIRustFRU_Tier23\defs_tier23.rs'
$bin = "$elf.bin"
$programElf = $elf
if ($Eval) { $bin = "$elf-eval.bin"; $programElf = "$elf-eval.elf" }
$ComPort = 'COM3'
# The baud rate is IPMC_CONSOLE_BAUD of crates\ipmc-defs\src\defs.rs, the same define the firmware
# reads; tools\defs.py is the one reader of that file outside the Rust build.
$DefsReader = Join-Path $root '..\..\tools\defs.py'
$BaudText = (& python -I $DefsReader IPMC_CONSOLE_BAUD)
if ($LASTEXITCODE -ne 0 -or -not $BaudText) { throw "IPMC_CONSOLE_BAUD not read by $DefsReader" }
$Baud = [int]$BaudText
$CommandSettleMs = 400          # wait after sending a command before reading the reply
$ReplyQuietMs = 600             # a reply is complete once the console has been silent this long
$CommandDeadlineMs = 60000      # overall time allowed to collect one command's reply (a full DEBUG DUMP of the capture log takes tens of seconds at 115200 baud)
$ReplyBytesMax = 131072         # byte cap per command reply (a full DEBUG DUMP is tens of kilobytes)
$FlashSettleSec = 3             # the debug interface programs the image after the copy
$RemountWaitSec = 10            # the volume may drop and return while programming
$OpenOcdTimeoutMs = 60000       # longest OpenOCD run (a program of the whole image takes a few seconds)
# What OpenOCD prints when it cannot open the ST-LINK (no probe, or no WinUSB driver bound to it).
$NoProbePattern = 'open failed|LIBUSB_ERROR|No ST-LINK detected|unable to open ST-Link|Error: no device found'
$BuildStart = $null             # set by Invoke-Build: the time the image build began

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
    $script:BuildStart = Get-Date
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
        # Gap fill 0xFF: the bytes between the vector table and the program, which include the
        # FRU store sector, read as erased flash (see the header).
        arm-none-eabi-objcopy -O binary --gap-fill 0xFF $elf $bin
        if ($LASTEXITCODE -ne 0) { throw "objcopy failed (exit $LASTEXITCODE)" }
        arm-none-eabi-size $elf
        if ($LASTEXITCODE -ne 0) { throw "size failed (exit $LASTEXITCODE)" }
        $len = (Get-Item $bin -ErrorAction Stop).Length
        if ($len -le 0) { throw "image is empty: ${bin}" }
        Write-Host "Image: $bin ($len bytes)"
        if ($Eval) {
            Write-Host "EVALUATION image: $bin (the IPMB stops after IPMC_EVAL_MINUTES of defs_tier23.rs)"
        }
    } finally {
        $env:IPMC_TIER23 = $savedTier23
        $env:IPMC_EVAL = $savedEval
        Pop-Location
    }
}

# Both flash paths program the image the build above just made: the .bin is rewritten by every
# build, so it must be newer than the build start, and the ELF it was made from must not be newer
# than it (a stale or half-built pair is refused before anything is programmed).
function Assert-FreshImage {
    $binItem = Get-Item $bin -ErrorAction Stop
    if ($binItem.Length -le 0) { throw "image is empty: ${bin}" }
    if ($binItem.LastWriteTime -lt $script:BuildStart) { throw "${bin} is older than this build: not the image just made" }
    $elfItem = Get-Item $programElf -ErrorAction Stop
    if ($elfItem.LastWriteTime -gt $binItem.LastWriteTime) { throw "${programElf} is newer than ${bin}: the pair is not one build" }
}

# Runs openocd with the given -c commands, waits at most $OpenOcdTimeoutMs, returns the exit code
# and the text it printed. A run that does not end in time is killed and is an error.
function Invoke-OpenOcd([string[]]$OcdCommands) {
    $argList = @('-f', 'interface/stlink.cfg', '-f', 'target/stm32f4x.cfg')
    foreach ($c in $OcdCommands) { $argList += @('-c', "`"$c`"") }
    $outFile = [System.IO.Path]::GetTempFileName()
    $errFile = [System.IO.Path]::GetTempFileName()
    try {
        $proc = Start-Process -FilePath 'openocd' -ArgumentList $argList -NoNewWindow -PassThru -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        $null = $proc.Handle   # keeps the process handle open so ExitCode is available after the exit
        if (-not $proc.WaitForExit($OpenOcdTimeoutMs)) {
            $proc.Kill()
            throw "openocd did not finish within $OpenOcdTimeoutMs ms"
        }
        $code = $proc.ExitCode
        $text = ((Get-Content $outFile -ErrorAction Stop) + (Get-Content $errFile -ErrorAction Stop)) -join "`n"
        return [pscustomobject]@{ Code = $code; Text = $text }
    } finally {
        Remove-Item $outFile, $errFile -ErrorAction SilentlyContinue
    }
}

# SWD: openocd programs the ELF (sections only, so the FRU store sector keeps its content),
# verifies it, resets the target and exits. Returns $true when programmed, $false when OpenOCD
# could not open the probe (the caller decides whether that is an error); any other failure throws.
function Invoke-SwdFlash {
    $image = $programElf.Replace('\', '/')
    Write-Host "Flashing over SWD: $programElf"
    $run = Invoke-OpenOcd @("program $image verify reset exit")
    if ($run.Code -eq 0) {
        Write-Host 'Flash OK (SWD: programmed, verified, reset)'
        return $true
    }
    if ($run.Text -match $NoProbePattern) {
        Write-Host $run.Text
        return $false
    }
    Write-Host $run.Text
    throw "SWD programming failed (openocd exit $($run.Code)); the board may hold a partial image"
}

function Invoke-DriveFlash {
    $vol = Get-Volume -ErrorAction Stop | Where-Object FileSystemLabel -like $VolumeLabel | Select-Object -First 1
    if (-not $vol -or -not $vol.DriveLetter) { throw "$VolumeLabel volume not found" }
    $drive = "$($vol.DriveLetter):\"
    Write-Host "Flashing by drive copy to $drive (the FRU store sector is erased by this image)"
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
    Write-Host 'Flash OK (drive copy done, drive present, no FAIL.TXT)'
}

function Invoke-Flash {
    if ($Drive -and $Swd) { throw '-Drive and -Swd together: choose one flash path' }
    Invoke-Build
    Assert-FreshImage
    if ($Drive) {
        Write-Host 'Flash path: drive copy (-Drive)'
        Invoke-DriveFlash
        return
    }
    if ($Swd) {
        Write-Host 'Flash path: SWD (-Swd)'
        Assert-Tool 'openocd'
        if (-not (Invoke-SwdFlash)) { throw 'SWD: OpenOCD found no ST-LINK probe it can open (WinUSB driver bound with Zadig?); -Swd does not fall back' }
        return
    }
    if (-not (Get-Command 'openocd' -ErrorAction SilentlyContinue)) {
        Write-Host 'Flash path: drive copy (OpenOCD is not installed)'
        Invoke-DriveFlash
        return
    }
    Write-Host 'Flash path: SWD (OpenOCD found)'
    if (Invoke-SwdFlash) { return }
    Write-Host 'Flash path: drive copy (OpenOCD reported no probe)'
    Invoke-DriveFlash
}

function Invoke-Console {
    $port = New-Object System.IO.Ports.SerialPort $ComPort, $Baud, 'None', 8, 'One'
    $port.ReadTimeout = $ReplyQuietMs
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
                Write-Warning "reply to '$cmd' cut off (deadline $CommandDeadlineMs ms or $ReplyBytesMax bytes; it was still arriving)"
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
