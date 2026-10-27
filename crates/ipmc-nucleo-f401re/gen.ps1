# gen.ps1 - build, flash and console helper for the NUCLEO-F401RE images: the bootloader (sectors 0-1)
# (ipmc-boot) and the runtime firmware (ipmc-nucleo-f401re). Layout and build: Documentation/
# ipmc_boot_plan.md 2, 3.5 and 5.
#   .\gen.ps1 build                  build both images and the board image (steps below)
#   .\gen.ps1 flash                  build, then copy ipmc-board.bin to the NOD_F401RE volume
#   .\gen.ps1 flash -Drive           the same (forced)
#   .\gen.ps1 flash -Swd             build, then program the patched runtime ELF over SWD with OpenOCD
#   .\gen.ps1 flash -Swd -Bootloader the same, and the bootloader ELF as well (bench only)
#   .\gen.ps1 console <cmd> [...]    open COM3, send each argument as a command, print the reply
#   -Eval                            with build or flash: the evaluation runtime (IPMC_EVAL=1): the IPMB
#                                    stops after IPMC_EVAL_MINUTES (defs_tier23.rs). Needs the
#                                    Tier23 folder. Every runtime file carries -eval in its name
#                                    (ipmc-board-eval.bin, ipmc-nucleo-f401re-eval.hex, ...), so it is
#                                    never mistaken for a full image. The bootloader has no evaluation
#                                    code: the cargo call for it always gets IPMC_EVAL=0, as does the
#                                    runtime without -Eval, so a build after an evaluation build
#                                    carries no evaluation code.
#   -Field                           with build or flash: the field images (IPMC_FIELD=1 for both cargo
#                                    calls): the boot countdown is IPMC_BOOT_COUNTDOWN_FIELD. Files
#                                    carry -field (ipmc-boot-field.bin, ipmc-board-field.bin, ...);
#                                    with -Eval the suffix is -eval-field. Restored after the build.
#   -VolumeLabel <pattern>           flash target volume label (default NOD_F401RE)
#   Tier-2/3 extension: build sets IPMC_TIER23 for cargo (unless already set) to 1 when
#   ..\..\..\IPMIRustFRU_Tier23\defs_tier23.rs exists and to 0 otherwise, so a folder that
#   appears or disappears after an earlier build takes effect (cargo reruns the ipmc-defs build
#   script when the variable changes). Set IPMC_TIER23 yourself to override.
# Build steps (the files are in target\thumbv7em-none-eabihf\release unless noted; <sfx> is the
# -eval and -field suffix):
#   1. cargo build --profile boot --bin ipmc-boot (IPMC_EVAL=0); objcopy -O binary --gap-fill 0xFF to
#      ipmc-boot<sfx>.bin; fwimage size (fails above 32 768 bytes, warns above 24 576).
#   2. cargo build --release --bin ipmc-nucleo-f401re (Tier23, Eval and Field as above).
#   3. objcopy -O binary --gap-fill 0xFF of the runtime ELF to ipmc-nucleo-f401re<sfx>.app.img (the
#      bytes from 0x0800_C000; the ELF holds nothing below it).
#   4. fwimage patch: fills the image length and the SHA-256 digest into the header placeholder
#      and writes the 64 header bytes (app_header<sfx>.bin).
#   5. objcopy --update-section .app_header=... gives the patched ELF ipmc-nucleo-f401re<sfx>.elf;
#      the image is made from it again and fwimage check runs the bootloader's own image check on
#      it (a failure stops the build).
#   6. objcopy -O ihex of the patched ELF: ipmc-nucleo-f401re<sfx>.hex (the file for LOAD).
#   7. fwimage combine: ipmc-board<sfx>.bin = the bootloader at 0, FFh to 0xC000 (the rest of BOOT and all of STORE), the runtime image.
#   8. Each file with its size; arm-none-eabi-size of both ELFs.
# No runtime-only .bin is written: the ST-LINK drive programs a .bin from 0x0800_0000, so it would
# land on the bootloader.
# Flash paths. What each writes and erases:
#   Drive copy (ipmc-board<sfx>.bin): everything. The bootloader (sectors 0-1), the FRU store (sector
#     2, written as FFh: the unit's FRU image and settings are erased, the owner accepts that) and
#     the runtime (sectors 3-7) are written; the board resets into the new bootloader.
#   SWD (-Swd), openocd -f interface/stlink.cfg -f target/stm32f4x.cfg -c "program <ELF> verify"
#     ... reset exit: only the sectors the ELF covers. The patched runtime ELF covers sectors 3
#     onward (the bootloader and the FRU store keep their content); with -Bootloader the
#     bootloader ELF (sectors 0-1) is programmed as well. OpenOCD is on PATH through winget (xPack),
#     and the ST-LINK debug interface needs the WinUSB driver (bound with Zadig, owner action).
#     A SWD program that starts and fails is an error, never a silent fallback. -Swd never falls
#     back to the drive copy, -Drive never uses OpenOCD; both together are refused. The script
#     prints which path it took.
#   Both paths program the images of the build this call just made (checked before anything is
#   programmed).
# Every failure exits non-zero with a message. "Flash OK" prints only after the programming, and
# for the drive copy the drive check and the FAIL.TXT check, all pass.
param(
    [Parameter(Position = 0, Mandatory = $true)][ValidateSet('build', 'flash', 'console')][string]$Action,
    [string]$VolumeLabel = 'NOD_F401RE',
    [switch]$Eval,
    [switch]$Field,
    [switch]$Drive,
    [switch]$Swd,
    [switch]$Bootloader,
    [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Commands
)

# Native tools (cargo, objcopy) write progress to stderr, so the global preference stays
# 'Continue' and their failures are caught by explicit $LASTEXITCODE checks. Every cmdlet that
# can fail carries -ErrorAction Stop so its failure is terminating.
$ErrorActionPreference = 'Continue'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$root = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $root '..\..')).Path
$releaseDir = Join-Path $root 'target\thumbv7em-none-eabihf\release'
$bootDir = Join-Path $root 'target\thumbv7em-none-eabihf\boot'
$tier23Header = Join-Path $root '..\..\..\IPMIRustFRU_Tier23\defs_tier23.rs'
# The file name suffixes: -eval for the evaluation runtime, -field for the field images.
$fieldSuffix = if ($Field) { '-field' } else { '' }
$evalSuffix = if ($Eval) { '-eval' } else { '' }
$suffix = $evalSuffix + $fieldSuffix
$cargoRuntimeElf = Join-Path $releaseDir 'ipmc-nucleo-f401re'
$cargoBootElf = Join-Path $bootDir 'ipmc-boot'
$bootElf = Join-Path $releaseDir "ipmc-boot$fieldSuffix.elf"
$bootBin = Join-Path $releaseDir "ipmc-boot$fieldSuffix.bin"
$appImg = Join-Path $releaseDir "ipmc-nucleo-f401re$suffix.app.img"
$headerBin = Join-Path $releaseDir "app_header$suffix.bin"
$programElf = Join-Path $releaseDir "ipmc-nucleo-f401re$suffix.elf"
$hex = Join-Path $releaseDir "ipmc-nucleo-f401re$suffix.hex"
$bin = Join-Path $releaseDir "ipmc-board$suffix.bin"
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

# Runs the host tool fwimage (crates\ipmc-boot, a host build) from the repository root; a refusal
# (exit code 1) or a usage error stops the build.
function Invoke-Fwimage([string[]]$FwArgs) {
    Push-Location $repo
    # The host build needs no evaluation code (and ipmc-defs accepts IPMC_EVAL=1 in a release build only).
    $savedEval = $env:IPMC_EVAL
    $env:IPMC_EVAL = '0'
    try {
        cargo run -q -p ipmc-boot --bin fwimage -- @FwArgs
        if ($LASTEXITCODE -ne 0) { throw "fwimage $($FwArgs[0]) failed (exit $LASTEXITCODE)" }
    } finally {
        $env:IPMC_EVAL = $savedEval
        Pop-Location
    }
}

# objcopy -O binary --gap-fill 0xFF: the bytes between output sections read as erased flash.
function Invoke-ObjcopyBinary([string]$Elf, [string]$Out) {
    arm-none-eabi-objcopy -O binary --gap-fill 0xFF $Elf $Out
    if ($LASTEXITCODE -ne 0) { throw "objcopy -O binary $Elf failed (exit $LASTEXITCODE)" }
}

function Write-File([string]$Path) {
    $len = (Get-Item $Path -ErrorAction Stop).Length
    if ($len -le 0) { throw "file is empty: ${Path}" }
    Write-Host ("  {0,-48} {1,9} bytes" -f (Split-Path $Path -Leaf), $len)
}

function Invoke-Build {
    Assert-Tool 'cargo'
    Assert-Tool 'arm-none-eabi-objcopy'
    Assert-Tool 'arm-none-eabi-size'
    Push-Location $root
    $savedTier23 = $env:IPMC_TIER23
    $savedEval = $env:IPMC_EVAL
    $savedField = $env:IPMC_FIELD
    $script:BuildStart = Get-Date
    try {
        if ($Eval) {
            if (-not (Test-Path $tier23Header -PathType Leaf)) { throw "-Eval needs the Tier23 folder: $tier23Header not found" }
            if ($env:IPMC_TIER23 -eq '0') { throw '-Eval needs the Tier-2/3 extension, but IPMC_TIER23 is 0' }
        }
        $env:IPMC_FIELD = if ($Field) { '1' } else { '0' }
        if (-not $env:IPMC_TIER23) {
            if (Test-Path $tier23Header -PathType Leaf) {
                $env:IPMC_TIER23 = '1'
                Write-Host "Tier-2/3 extension: built from $((Resolve-Path (Split-Path $tier23Header)).Path)"
            } else {
                $env:IPMC_TIER23 = '0'
                Write-Host 'Tier-2/3 extension: not present, Tier-1 build'
            }
        }

        # 1. The bootloader: no evaluation code in it, whatever the runtime is.
        $env:IPMC_EVAL = '0'
        cargo build --profile boot --bin ipmc-boot
        if ($LASTEXITCODE -ne 0) { throw "cargo build (bootloader) failed (exit $LASTEXITCODE)" }
        # The release folder may not exist yet (the bootloader builds into the boot profile folder
        # and the runtime build comes after it). Copy-Item keeps the source time, so the copy gets
        # the time of the copy (a cached bootloader build still passes Assert-FreshImage), and its
        # hash must equal Cargo's output.
        New-Item -ItemType Directory -Force -Path (Split-Path $bootElf -Parent) | Out-Null
        Copy-Item $cargoBootElf $bootElf -Force -ErrorAction Stop
        (Get-Item $bootElf -ErrorAction Stop).LastWriteTime = Get-Date
        if ((Get-FileHash $cargoBootElf).Hash -ne (Get-FileHash $bootElf).Hash) {
            throw "${bootElf} differs from Cargo's output ${cargoBootElf}"
        }
        Invoke-ObjcopyBinary $bootElf $bootBin
        Invoke-Fwimage @('size', $bootBin)

        # 2. The runtime.
        $env:IPMC_EVAL = if ($Eval) { '1' } else { '0' }
        cargo build --release --bin ipmc-nucleo-f401re
        if ($LASTEXITCODE -ne 0) { throw "cargo build (runtime) failed (exit $LASTEXITCODE)" }

        # 3 to 5. The image, its header (length and SHA-256), the patched ELF, the build check.
        Invoke-ObjcopyBinary $cargoRuntimeElf $appImg
        Invoke-Fwimage @('patch', $appImg, $headerBin)
        arm-none-eabi-objcopy --update-section ".app_header=$headerBin" $cargoRuntimeElf $programElf
        if ($LASTEXITCODE -ne 0) { throw "objcopy --update-section failed (exit $LASTEXITCODE)" }
        Invoke-ObjcopyBinary $programElf $appImg
        Invoke-Fwimage @('check', $appImg)

        # 6 and 7. The LOAD file and the board image.
        arm-none-eabi-objcopy -O ihex $programElf $hex
        if ($LASTEXITCODE -ne 0) { throw "objcopy -O ihex failed (exit $LASTEXITCODE)" }
        Invoke-Fwimage @('combine', $bootBin, $appImg, $bin)

        # 8. The files and the sections.
        Write-Host 'Files:'
        foreach ($f in @($bootBin, $bootElf, $appImg, $programElf, $hex, $bin)) { Write-File $f }
        arm-none-eabi-size $bootElf $programElf
        if ($LASTEXITCODE -ne 0) { throw "size failed (exit $LASTEXITCODE)" }
        Write-Host "Board image: $bin (flash this file by the drive copy)"
        if ($Eval) {
            Write-Host "EVALUATION image: $bin (the IPMB stops after IPMC_EVAL_MINUTES of defs_tier23.rs)"
        }
        if ($Field) {
            Write-Host "FIELD images: IPMC_BOOT_COUNTDOWN_FIELD applies"
        }
    } finally {
        $env:IPMC_TIER23 = $savedTier23
        $env:IPMC_EVAL = $savedEval
        $env:IPMC_FIELD = $savedField
        Pop-Location
    }
}

# Both flash paths program the images the build above just made: the files are rewritten by every
# build, so they must be newer than the build start, and the ELFs they were made from must not be
# newer than the board image (a stale or half-built set is refused before anything is programmed).
# For the bootloader ELF this checks the copy made by Invoke-Build: the copy is stamped with the time
# of the copy (a cached Cargo build is not rebuilt), and Invoke-Build has already refused a copy
# whose hash differs from Cargo's output, so a copy older than the build start is a copy that did
# not happen in this run.
function Assert-FreshImage {
    $files = @($bin, $programElf)
    if ($Bootloader) { $files += $bootElf }
    foreach ($f in $files) {
        $item = Get-Item $f -ErrorAction Stop
        if ($item.Length -le 0) { throw "image is empty: ${f}" }
        if ($item.LastWriteTime -lt $script:BuildStart) { throw "${f} is older than this build: not the image just made" }
    }
    $binItem = Get-Item $bin -ErrorAction Stop
    foreach ($f in @($programElf, $bootElf)) {
        if ((Get-Item $f -ErrorAction Stop).LastWriteTime -gt $binItem.LastWriteTime) { throw "${f} is newer than ${bin}: the set is not one build" }
    }
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

# SWD: openocd programs the patched runtime ELF (sectors 3 onward only; the bootloader and the
# FRU store sector keep their content), and with -Bootloader the bootloader ELF (sectors 0-1),
# verifies, resets the target and exits. Returns $true when programmed, $false when OpenOCD could
# not open the probe (the caller decides whether that is an error); any other failure throws.
function Invoke-SwdFlash {
    $ocdCommands = @("program $($programElf.Replace('\', '/')) verify")
    Write-Host "Flashing over SWD: $programElf"
    if ($Bootloader) {
        $ocdCommands += "program $($bootElf.Replace('\', '/')) verify"
        Write-Host "Flashing over SWD: $bootElf"
    }
    $run = Invoke-OpenOcd ($ocdCommands + @('reset', 'exit'))
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
    Write-Host "Flashing by drive copy to $drive (the bootloader, the runtime and the FRU store sector are written by this image)"
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
    if ($Bootloader -and -not $Swd) { throw '-Bootloader belongs to -Swd (the drive copy always writes the bootloader)' }
    Invoke-Build
    Assert-FreshImage
    if ($Swd) {
        Write-Host 'Flash path: SWD (-Swd)'
        Assert-Tool 'openocd'
        if (-not (Invoke-SwdFlash)) { throw 'SWD: OpenOCD found no ST-LINK probe it can open (WinUSB driver bound with Zadig?); -Swd does not fall back' }
        return
    }
    Write-Host 'Flash path: drive copy of the board image'
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
