// Drawing set: ipmc-software (7 sheets) for Documentation/ipmc_software_spec.md.
// Build: node diagrams/src/ipmc-software.js  ->  diagrams/out/ipmc-software.pptx
// Fills: white = no_std code shared by firmware and host build, gray = firmware only (STM32),
// blue = host only (PC), yellow = configuration or data file.
const path = require("path");
const { newDeck, helpers, FILL } = require("../lib/engslides");

const pres = newDeck("IPMC software drawings");
const { box, container, wire, label, notes, legend, frame } = helpers(pres);
const W = FILL.white, G = FILL.gray, B = FILL.blue, Y = FILL.yellow;
const LEG = [["no_std code, firmware and host", W], ["Firmware only (STM32)", G], ["Host only (PC)", B], ["Configuration or data file", Y]];
const cx = (x, w) => x + w / 2;
const cy = (y, h) => y + h / 2;

// ---------------------------------------------------------------- Sheet 1
{
  const s = pres.addSlide();
  container(s, 0.6, 0.5, 3.3, 3.3, "ipmc-nucleo-f401re (own workspace)");
  container(s, 4.2, 0.5, 8.55, 6.2, "Root cargo workspace");
  // the one F401 crate: two binaries over one library
  box(s, 1.0, 0.95, 2.7, 0.8, "Bootloader binary", ["src/boot/, sectors 0-1", "start sequence, session glue"], G);
  box(s, 1.0, 1.95, 2.7, 0.8, "Runtime binary", ["src/main.rs, src/drivers/", "ipmc-runtime link, console, app_port"], G);
  box(s, 1.0, 3.0, 2.7, 0.6, "Library src/lib.rs, src/bsp/", ["board support of both images"], G);
  wire(s, 1.0, 1.35, 0.8, 1.35, { end: false });
  wire(s, 0.8, 1.35, 0.8, 3.3, { end: false });
  wire(s, 0.8, 3.3, 1.0, 3.3);
  wire(s, 1.0, 2.35, 0.8, 2.35, { end: false });
  const c1 = 4.5, c2 = 9.1, w = 3.3, n = 2.2;
  box(s, c1, 0.95, w, 0.8, "ipmc-boot", ["bootloader logic, no_std, host-tested", "header check, session, Intel HEX, trace", "bin fwimage: patch, check, combine"], W);
  box(s, c1, 2.0, n, 0.55, "ipmc-app", ["application port protocol"], W);
  box(s, c1, 2.8, n, 0.55, "ipmc-menu", ["console menu engine"], W);
  box(s, c1, 3.6, n, 0.65, "ipmc-log", ["capture log and", "DEBUG logging code"], W);
  box(s, 6.9, 2.8, 1.6, 0.75, "ipmc-ext", ["bridge crate: Active is", "NoExtension or the", "extension"], W);
  box(s, c2, 0.85, w, 0.6, "ipmc-emu", ["x86 emulator", "replay, serve, sdr"], B);
  box(s, c2, 1.7, w, 0.6, "ipmc-sensors", ["sensor and board I/O HAL", "hal, eval, drivers, board table"], W);
  box(s, c2, 2.6, w, 0.85, "ipmc-core", ["all IPMI logic", "codec, link layer, commands, SDR,", "sensors, FRU state, self test"], W);
  box(s, c2, 3.75, w, 0.6, "ipmc-fru", ["FRU records, scratchpad, image,", "flash store log"], W);
  box(s, c2, 4.65, w, 0.6, "ipmc-wire", ["IPMI byte primitives", "checksum, builder, CRC-32, SHA-256"], W);
  box(s, c1, 4.7, n, 0.6, "ipmc-defs", ["typed constants (defs.rs)", "and compile-time guards"], W);
  box(s, c1, 5.9, n, 0.55, "defs.rs", ["build configuration"], Y);
  box(s, 7.0, 5.9, 5.4, 0.55, "Tier-2/3 extension", ["(commercial release build, private sibling folder)"], W);
  const x2 = cx(c2, w);
  wire(s, x2, 1.45, x2, 1.7);
  wire(s, x2, 2.3, x2, 2.6);
  wire(s, x2, 3.45, x2, 3.75);
  wire(s, x2, 4.35, x2, 4.65);
  // firmware crate to ipmc-boot (bootloader binary) and ipmc-app (runtime binary)
  wire(s, 3.7, 1.35, c1, 1.35);
  wire(s, 3.7, 2.275, c1, 2.275);
  // ipmc-app uses ipmc-core
  wire(s, c1 + n, 2.275, 8.75, 2.275, { end: false });
  wire(s, 8.75, 2.275, 8.75, 2.9, { end: false });
  wire(s, 8.75, 2.9, c2, 2.9);
  // ipmc-ext uses ipmc-core
  wire(s, 8.5, 3.175, c2, 3.175);
  // defs.rs to ipmc-defs, extension folder to ipmc-ext
  wire(s, 5.6, 5.9, 5.6, 5.3, { dash: "dash" });
  label(s, 5.7, 5.55, 1.3, "lib.rs: mod defs");
  wire(s, 7.7, 3.55, 7.7, 5.9, { dash: "dash" });
  label(s, 7.8, 4.9, 1.2, "#[path] mount,");
  label(s, 7.8, 5.08, 1.2, "when found");
  notes(s, 0.6, 3.95, 3.5, 2.8, [
    "Every crate reads defs.rs values as ipmc_defs::NAME; those arrows are not drawn.",
    "The runtime binary also uses ipmc-core, -fru, -menu, -log, -sensors, -ext, -wire and -defs. The bootloader binary uses ipmc-boot and the library only.",
    "ipmc-boot uses ipmc-fru (store sector), -wire (SHA-256, checksum), -menu (line editor) and -defs. ipmc-log uses ipmc-core and -defs. ipmc-emu uses core, fru, sensors, wire, ext and defs. ipmc-app uses core, fru, wire and defs.",
    "ipmc-ext uses ipmc-core and -wire. ipmc-defs build.rs finds the extension folder; without it the build is Tier-1.",
    "The firmware crate builds on its own (thumbv7em-none-eabihf). Every other crate forbids unsafe code; ipmc-emu is a host std binary.",
  ]);
  legend(s, 0.6, 6.85, LEG, 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 2
{
  const s = pres.addSlide();
  container(s, 2.6, 0.55, 2.3, 3.05, "Interrupt context");
  container(s, 5.15, 0.55, 7.6, 5.1, "Main loop (thread)");
  const bw = 1.4, bh = 0.8;
  const c = [5.45, 7.25, 9.05, 10.85];
  const r1 = 1.0, r2 = 2.5, r3 = 4.0;
  // off-board buses
  box(s, 0.9, 2.15, 1.25, 0.6, "IPMB-A", ["I2C1, PB8 / PB9"]);
  box(s, 0.9, 3.05, 1.25, 0.6, "IPMB-B", ["I2C3, PA8 / PB4"]);
  // interrupt context
  box(s, 2.9, r1, 1.7, bh, "Frame queue per bus", ["SPSC,", "IPMB_RX_QUEUE_DEPTH"], G);
  box(s, 2.9, r2, 1.7, bh, "I2C slave interrupt", ["i2c_hw.rs, EV and ER", "bytes to frame,", "STOP time"], G);
  // main loop top row
  box(s, c[0], r1, bw, bh, "drain_rx", ["both queues,", "STOP-time order"], G);
  box(s, c[1], r1, bw, bh, "Link::on_frame", ["bus enabled,", "general call, parse,", "address"]);
  box(s, c[2], r1, bw, bh, "Ingress FIFO", ["IPMB_INGRESS_DEPTH"]);
  box(s, c[3], r1, bw, bh, "Link::poll", ["duplicate cache", "lookup"]);
  // middle row
  box(s, c[1], r2, bw, bh, "Context", ["FRU store, SDR,", "sensors, FRU state,", "self test, link view"]);
  box(s, c[2], r2, bw, bh, "Handlers", ["cmd/app, se,", "storage, vita"]);
  box(s, c[3], r2, bw, bh, "dispatch", ["LUN, VSO byte,", "command table"]);
  // bottom row
  box(s, c[0], r3, bw, bh, "IPMB master", ["bit-bang or hardware", "I2C, write_retrying"], G);
  box(s, c[1], r3, bw, bh, "Link::next_tx", ["bus choice:", "alternate or arrival"]);
  box(s, c[2], r3, bw, bh, "Egress queue", ["IPMB_TXQ_DEPTH"]);
  const y1 = cy(r1, bh), y2 = cy(r2, bh), y3 = cy(r3, bh);
  // receive path
  wire(s, 2.15, 2.45, 2.45, 2.45, { end: false });
  wire(s, 2.15, 3.35, 2.45, 3.35, { end: false });
  wire(s, 2.45, 2.45, 2.45, 3.35, { end: false });
  wire(s, 2.45, 2.9, 2.9, 2.9);
  wire(s, 3.75, r2, 3.75, r1 + bh);
  wire(s, 4.6, y1, c[0], y1);
  wire(s, c[0] + bw, y1, c[1], y1);
  wire(s, c[1] + bw, y1, c[2], y1);
  wire(s, c[2] + bw, y1, c[3], y1);
  wire(s, cx(c[3], bw), r1 + bh, cx(c[3], bw), r2);
  wire(s, c[3], y2, c[2] + bw, y2);
  wire(s, c[2], y2, c[1] + bw, y2, { start: true });
  // transmit path
  wire(s, cx(c[2], bw), r2 + bh, cx(c[2], bw), r3);
  label(s, cx(c[2], bw) + 0.08, (r2 + bh + r3) / 2 - 0.09, 1.6, "response, cached by poll");
  wire(s, c[2], y3, c[1] + bw, y3);
  wire(s, c[1], y3, c[0] + bw, y3);
  wire(s, c[0], y3, 0.7, y3, { end: false });
  wire(s, 0.7, y3, 0.7, 2.45, { end: false });
  wire(s, 0.7, 2.45, 0.9, 2.45);
  wire(s, 0.7, 3.35, 0.9, 3.35);
  label(s, 2.0, y3 - 0.2, 1.2, "master write");
  // cache-hit path
  wire(s, c[3] + bw, y1, 12.5, y1, { end: false });
  wire(s, 12.5, y1, 12.5, y3, { end: false });
  wire(s, 12.5, y3, c[2] + bw, y3);
  label(s, 10.6, y3 - 0.2, 1.8, "cached response");
  // result path
  const xm = cx(c[0], bw), xn = cx(c[1], bw);
  wire(s, xm, r3 + bh, xm, 5.4, { end: false });
  wire(s, xm, 5.4, xn, 5.4, { end: false });
  wire(s, xn, 5.4, xn, r3 + bh);
  label(s, xm + 0.05, 5.0, xn - xm - 0.1, "on_tx_result: pop, or resend", 7.5, "center");
  label(s, xm + 0.05, 5.18, xn - xm - 0.1, "on the other bus", 7.5, "center");
  notes(s, 0.6, 4.75, 4.4, 1.9, [
    "No IPMI code runs in an interrupt.",
    "One master write per main-loop pass, at most T2 + T1 = 80 ms.",
    "The fault tracker (ipmc-runtime link.rs) and the FRU flash log save run in the same pass and are on sheet 3.",
    "The same Link, dispatch and handlers run in the x86 emulator, with the interrupt context replaced by the emulator frame input.",
    "IPMB-A and IPMB-B are off-board bus hardware, not code.",
  ]);
  legend(s, 0.6, 6.85, LEG.slice(0, 2), 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 3
{
  const s = pres.addSlide();
  const rc = 1.7; // upper row center
  box(s, 0.6, 0.6, 1.9, 0.6, "ADC1", ["VREFINT, die temperature,", "A0-A3"], G);
  box(s, 0.6, 1.35, 1.9, 0.75, "Board GPIO", ["PGOOD, FAULT_N,", "OVERTEMP_N,", "PAYLOAD_RESET_N"], G);
  box(s, 0.6, 2.25, 1.9, 0.6, "Simulators", ["drivers/sim,", "SIM console group"]);
  wire(s, 2.5, 0.9, 2.75, 0.9, { end: false });
  wire(s, 2.5, 1.725, 2.75, 1.725, { end: false });
  wire(s, 2.5, 2.55, 2.75, 2.55, { end: false });
  wire(s, 2.75, 0.9, 2.75, 2.55, { end: false });
  box(s, 3.0, 1.3, 1.5, 0.8, "Routing", ["routing.rs, per", "defs.rs", "source switch"]);
  wire(s, 2.75, rc, 3.0, rc);
  box(s, 4.9, 1.25, 1.8, 0.9, "Monitor", ["monitor.rs: sample,", "power state machine,", "PWR_EN drive, trace lines"]);
  wire(s, 4.5, rc, 4.9, rc);
  box(s, 5.2, 2.6, 1.2, 0.5, "PWR_EN", ["PB5"], G);
  wire(s, 5.8, 2.15, 5.8, 2.6);
  box(s, 7.1, 1.3, 1.5, 0.8, "SensorSnapshot", ["rails, temperature,", "inputs, power,", "summaries"]);
  wire(s, 6.7, rc, 7.1, rc);
  box(s, 9.0, 1.3, 1.85, 0.8, "Context::update_sensors", ["stores the snapshot,", "steps the FRU", "state machine"]);
  wire(s, 8.6, rc, 9.0, rc);
  box(s, 11.2, 1.3, 1.5, 0.8, "Get Sensor Reading", ["sensors 0, 2-4;", "1: link view"]);
  wire(s, 10.85, rc, 11.2, rc);
  box(s, 9.0, 2.6, 1.85, 0.5, "FRU state machine", ["M0, M1, M4, M6"]);
  wire(s, 9.925, 2.1, 9.925, 2.6);
  // FRU store row
  const L = 3.6, h = 0.75, ly = L + h / 2;
  box(s, 0.6, L, 1.8, h, "Write FRU Data handler", ["storage command"]);
  box(s, 3.0, L, 1.8, h, "FruStore", ["RAM scratchpad,", "dirty mark"]);
  box(s, 5.4, L, 1.8, h, "Persist::poll", ["quiet time, NVMRO,", "erase gate"]);
  box(s, 7.8, L, 1.8, h, "PausedErase", ["IPMB slaves off", "during an erase"], G);
  box(s, 10.2, L, 2.2, h, "Flash log", ["drivers/flash.rs, sector 2 only", "0x0800_8000..0x0800_BFFF"], G);
  wire(s, 2.4, ly, 3.0, ly);
  wire(s, 4.8, ly, 5.4, ly);
  wire(s, 7.2, ly, 7.8, ly);
  wire(s, 9.6, ly, 10.2, ly);
  box(s, 3.0, 2.6, 1.8, 0.5, "Read FRU Data handler", ["storage command"]);
  wire(s, 3.9, L, 3.9, 3.1);
  box(s, 5.4, 4.95, 1.8, 0.5, "Persist::boot", []);
  wire(s, 11.3, L + h, 11.3, 5.2, { end: false });
  wire(s, 11.3, 5.2, 7.2, 5.2);
  wire(s, 5.4, 5.2, 3.9, 5.2, { end: false });
  wire(s, 3.9, 5.2, 3.9, L + h);
  label(s, 3.95, 5.0, 1.4, "boot: newest valid record");
  notes(s, 0.6, 5.75, 12.0, 0.95, [
    "One sample every IPMC_SENSOR_PERIOD_MS.",
    "A save waits IPMC_FRU_PERSIST_DELAY_MS after the last write; an erase also waits for a quiet bus (IPMC_FRU_ERASE_QUIET_MS).",
    "Nothing is written to flash while NVMRO is asserted.",
    "The emulator keeps the FRU store in RAM only.",
  ]);
  legend(s, 0.6, 6.85, LEG.slice(0, 2), 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 4
{
  const s = pres.addSlide();
  container(s, 0.6, 0.5, 9.1, 4.8, "PC (host)");
  const tx = 4.0, tw = 2.2;
  box(s, 0.9, 2.1, 2.3, 0.7, "board_test_report.py", ["runs the three tools", "and gen.ps1 console"], B);
  box(s, tx, 0.95, tw, 0.6, "python-ipmi ipmitool", ["named and raw commands"], B);
  box(s, tx, 2.15, tw, 0.6, "chassis_manager.py", ["discovery, FRU, SDR, polling"], B);
  box(s, tx, 3.35, tw, 0.6, "run_conformance.py", ["spec-only Tier-1 suite,", "adapter.py"], B);
  box(s, 7.75, 2.7, 1.6, 0.7, "ipmc-emu serve", ["TCP 127.0.0.1:5470,", "IPMB-A and B"], B);
  box(s, 7.75, 4.2, 1.6, 0.7, "ipmc-emu replay", ["captured traffic,", "vectors/sbc_ab.vec"], B);
  box(s, 10.95, 0.95, 1.8, 0.6, "Aardvark", ["USB to I2C, pull-ups on"]);
  box(s, 10.95, 2.7, 1.8, 0.8, "NUCLEO-F401RE", ["IPMC firmware,", "IPMB 82h"], G);
  box(s, 10.95, 4.5, 1.8, 0.7, "ST-LINK", ["USB: COM3 console,", "NOD_F401RE drive"]);
  // board_test_report runs the three tools (dashed control bus)
  wire(s, 3.2, 2.45, 3.55, 2.45, { dash: "dash", end: false });
  wire(s, 3.55, 1.25, 3.55, 3.65, { dash: "dash", end: false });
  wire(s, 3.55, 1.25, tx, 1.25, { dash: "dash" });
  wire(s, 3.55, 2.45, tx, 2.45, { dash: "dash" });
  wire(s, 3.55, 3.65, tx, 3.65, { dash: "dash" });
  // tools to emulator
  wire(s, tx + tw, 2.45, 6.55, 2.45, { end: false });
  wire(s, tx + tw, 3.65, 6.55, 3.65, { end: false });
  wire(s, 6.55, 2.45, 6.55, 3.65, { end: false });
  wire(s, 6.55, 3.05, 7.75, 3.05);
  label(s, 6.65, 2.85, 1.1, "TCP line protocol");
  // ipmitool to Aardvark
  wire(s, tx + tw, 1.25, 10.95, 1.25);
  label(s, 9.9, 1.05, 0.9, "USB");
  // Aardvark to NUCLEO, ST-LINK to NUCLEO
  wire(s, 11.85, 1.55, 11.85, 2.7, { start: true });
  label(s, 10.2, 2.0, 1.55, "I2C: IPMB-A, PB8 / PB9", 7.5, "right");
  wire(s, 11.85, 3.5, 11.85, 4.5, { start: true });
  label(s, 10.2, 3.9, 1.55, "USART2, SWD", 7.5, "right");
  // PC to ST-LINK
  wire(s, 9.7, 4.85, 10.95, 4.85);
  label(s, 9.8, 4.47, 1.1, "flash: copy .bin;");
  label(s, 9.8, 4.65, 1.1, "console COM3");
  notes(s, 0.6, 5.7, 12.0, 0.95, [
    "One Aardvark drives IPMB-A only, so IPMB-B on hardware is not tested on this bench. chassis_manager.py and run_conformance.py also reach the Aardvark through their aardvark transport (not drawn).",
    "The conformance tests are written from the specification texts only and reach the IPMC through adapter.py.",
    "vectors/sbc_ab.vec is local only and not in the repository.",
    "White blocks on this sheet are bench hardware, not code from this repository.",
    "The firmware update tool (fwupdate.py) and the application port client (app_cli.py) are on sheets 6 and 7.",
  ]);
  legend(s, 0.6, 6.85, LEG.slice(1, 3), 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 5
{
  const s = pres.addSlide();
  // flash map, column A
  box(s, 0.7, 0.7, 3.7, 0.8, "BOOT, sectors 0-1", ["0x0800_0000 .. 0x0800_7FFF, 32 KB", "the bootloader ipmc-boot"], G);
  box(s, 0.7, 1.75, 3.7, 0.8, "STORE, sector 2", ["0x0800_8000 .. 0x0800_BFFF, 16 KB", "the FRU flash store, nothing else"], Y);
  container(s, 0.7, 2.8, 3.7, 3.75, "APP, sectors 3-7, 0x0800_C000 .. 0x0807_FFFF");
  box(s, 0.95, 3.15, 3.2, 0.6, "Vector table", ["0x0800_C000, 512-byte aligned"], G);
  box(s, 0.95, 3.95, 3.2, 0.6, "Image header, 64 bytes", ["0x0800_C200: length, version, digest"], Y);
  box(s, 0.95, 4.75, 3.2, 0.6, "Program", ["text, rodata, .data load image", "ends at 0x0800_C000 + length"], G);
  box(s, 0.95, 5.55, 3.2, 0.7, "Unused flash", ["reads FFh after ERASE APP"], W);
  // writers, column B
  box(s, 5.0, 0.7, 3.2, 0.8, "Writers of BOOT", ["SWD of ipmc-boot.elf (bench only)", "drive copy of ipmc-board.bin"], B);
  box(s, 5.0, 1.75, 3.2, 0.8, "Writers of STORE", ["runtime drivers/flash.rs", "bootloader ERASE FRU, drive copy (FFh)"], G);
  box(s, 5.0, 3.55, 3.2, 1.0, "Writers of APP", ["bootloader LOAD (Intel HEX records)", "SWD of the patched runtime ELF", "drive copy of ipmc-board.bin"], G);
  wire(s, 5.0, 1.1, 4.4, 1.1);
  wire(s, 5.0, 2.15, 4.4, 2.15);
  wire(s, 5.0, 4.05, 4.4, 4.05);
  // RAM and the boot request, column C
  box(s, 9.0, 0.7, 3.6, 0.6, "Runtime firmware", ["DEBUG BOOTLOADER, app BOOTLOADER"], G);
  box(s, 9.0, 1.7, 3.6, 0.8, "NOINIT, 0x2000_0000, 256 B", ["offset 0: boot request, four words", "offset 0x10: runtime kept words"], Y);
  box(s, 9.0, 2.9, 3.6, 0.6, "Bootloader", ["reads the words after a software reset"], G);
  box(s, 9.0, 3.9, 3.6, 1.0, "RAM, 0x2000_0100 .. 0x2001_7FFF", ["95.75 KB: statics and stack of the", "image that runs; stack reserve 35 KB", "runtime, 8 KB bootloader"], W);
  wire(s, 10.8, 1.3, 10.8, 1.7);
  label(s, 10.9, 1.4, 1.7, "write 4 words, reset");
  wire(s, 10.8, 2.5, 10.8, 2.9);
  label(s, 10.9, 2.6, 1.7, "read, clear check word");
  notes(s, 5.0, 5.0, 7.7, 1.75, [
    "bsp/flash_regs.rs refuses sectors 0 and 1 and every address below 0x0800_8000 for every caller; ipmc_boot::geometry::load_allowed lets LOAD write inside APP only. Two checks stand between any command and the bootloader sectors.",
    "Sector 3 is 16 KB (0x0800_C000), sector 4 is 64 KB (0x0801_0000), sectors 5, 6 and 7 are 128 KB each (0x0802_0000, 0x0804_0000, 0x0806_0000). ERASE APP erases sector 3 first, so a power cut leaves an image that fails its check.",
    "ipmc-board.bin holds the bootloader at offset 0, FFh up to 0xC000 and the runtime from 0xC000; the drive copy therefore erases the stored FRU image.",
  ]);
  legend(s, 0.6, 6.85, [["Firmware code", G], ["Data", Y], ["Host tool", B], ["Region without content", W]], 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 6
{
  const s = pres.addSlide();
  container(s, 0.6, 0.5, 3.0, 4.7, "PC (host)");
  box(s, 0.85, 1.0, 2.5, 0.7, "gen.ps1 build", ["fwimage patch and check,", "objcopy"], B);
  box(s, 0.85, 2.1, 2.5, 0.7, "ipmc-nucleo-f401re.hex", ["patched runtime, SHA-256", "in the image header"], Y);
  box(s, 0.85, 3.2, 2.5, 1.2, "tools/fwupdate/fwupdate.py", ["pre-check: header, vectors, digest", "ERASE APP, LOAD, 32-byte records", "resend, resume, VERIFY, RUN"], B);
  wire(s, 2.1, 1.7, 2.1, 2.1);
  wire(s, 2.1, 2.8, 2.1, 3.2);
  // board
  container(s, 4.2, 0.5, 8.55, 5.0, "NUCLEO-F401RE, bootloader ipmc-boot (sectors 0-1)");
  const col = [4.5, 6.2, 7.9, 9.6, 11.3], bw = 1.4, bh = 0.9;
  const r1 = 1.1, r2 = 3.25;
  // band 1: the image check at every reset
  box(s, col[0], r1, bw, bh, "Reset", ["bootloader starts", "first, clock, console"], G);
  box(s, col[1], r1, bw, bh, "main.rs", ["pin PC10, boot", "request words"], G);
  box(s, col[2], r1, bw, bh, "header::check", ["header, vectors,", "SHA-256"], W);
  box(s, col[3], r1, bw, bh, "decide", ["start, countdown", "or stay"], W);
  box(s, col[4], r1, bw, bh, "start.rs", ["start_runtime:", "VTOR, MSP, jump"], G);
  for (let i = 0; i < 4; i++) wire(s, col[i] + bw, r1 + bh / 2, col[i + 1], r1 + bh / 2);
  // band 2: the update session
  box(s, col[0], r2, bw, bh, "bsp/uart.rs", ["USART2 (CLI) or USART1", "(APP), 115200 8N1"], G);
  box(s, col[1], r2, bw, bh, "serve.rs", ["line editor per port,", "WAKEUP rule"], G);
  box(s, col[2], r2, bw, bh, "Session", ["commands.rs table,", "ERASE, LOAD, VERIFY"], W);
  box(s, col[3], r2, bw, bh, "Loader::record", ["parse, range, erased", "check, program, compare"], W);
  box(s, col[4], r2, bw, bh, "FlashImage", ["FlashRegs: erase", "sector, program"], G);
  for (let i = 0; i < 4; i++) wire(s, col[i] + bw, r2 + bh / 2, col[i + 1], r2 + bh / 2, { start: true });
  // check and session
  wire(s, col[2] + bw / 2, r2, col[2] + bw / 2, r1 + bh, { start: true });
  label(s, col[2] + bw / 2 + 0.08, 2.3, 1.1, "VERIFY, RUN");
  label(s, col[2] + bw / 2 + 0.08, 2.48, 1.1, "end of LOAD");
  // host to board and flash
  wire(s, 3.35, r2 + bh / 2, col[0], r2 + bh / 2, { start: true });
  box(s, col[4], 4.5, bw, 0.7, "Flash APP", ["sectors 3-7"], G);
  wire(s, col[4] + bw / 2, r2 + bh, col[4] + bw / 2, 4.5, { start: true });
  notes(s, 0.6, 5.55, 12.0, 1.2, [
    "Start-up (upper row): every reset runs the check; a valid image with no pin, no request and no countdown starts at once. A countdown or a stay listens on both ports through serve.rs; WAKEUP on one port opens the session there and stops the other UART.",
    "Update (lower row): stop-and-wait, one record in flight, answered OK or ERR before the next. A record that fails FORMAT, CHECKSUM, LINE_TOO_LONG or LINE_DAMAGED is sent again; ERR ABORTED resumes with LOAD. VERIFY returns the computed SHA-256, which fwupdate.py compares with the file.",
    "header::check reads the flash through ImageSource (FlashImage). No IPMB service runs while the bootloader runs. The host PC reaches CLI through the ST-LINK virtual COM port and APP through a USB-serial adapter.",
  ]);
  legend(s, 0.6, 6.85, [["Firmware code", G], ["ipmc-boot library", W], ["Host tool", B], ["Data file", Y]], 3.0);
  frame(s);
}

// ---------------------------------------------------------------- Sheet 7
{
  const s = pres.addSlide();
  const y = 1.3, h = 0.9, cy1 = y + h / 2;
  box(s, 0.6, y, 1.4, h, "Payload processor", ["board hardware"], W);
  box(s, 2.7, y, 1.4, h, "bsp/uart.rs", ["USART1 PB6 TX,", "PB7 RX", "RX 256, TX 1024"], G);
  box(s, 4.6, y, 1.5, h, "app_port.rs", ["AppPort::poll", "echo, answer,", "prompt, drain"], G);
  container(s, 6.65, 0.8, 5.85, 1.7, "ipmc-app (reference crate, no_std)");
  box(s, 6.85, y, 1.15, h, "LineEditor", ["line.rs", "echo, line", "end, damage"], W);
  box(s, 8.25, y, 1.15, h, "engine::run", ["engine.rs", "tag, row,", "final line"], W);
  box(s, 9.65, y, 1.15, h, "COMMANDS", ["commands.rs", "handlers"], W);
  box(s, 11.05, y, 1.15, h, "AppHost", ["host.rs", "the trait"], W);
  wire(s, 2.0, cy1, 2.7, cy1, { start: true });
  label(s, 2.05, cy1 - 0.2, 0.65, "8N1", 7.5, "center");
  wire(s, 4.1, cy1, 4.6, cy1, { start: true });
  wire(s, 6.1, cy1, 6.85, cy1, { start: true });
  wire(s, 8.0, cy1, 8.25, cy1, { start: true });
  wire(s, 9.4, cy1, 9.65, cy1, { start: true });
  wire(s, 10.8, cy1, 11.05, cy1, { start: true });
  // LinkHost implements AppHost
  box(s, 10.625, 3.3, 2.0, 0.8, "LinkHost", ["app_port.rs: implements", "AppHost over the link layer"], G);
  wire(s, 11.625, 2.2, 11.625, 3.3, { start: true });
  label(s, 11.7, 2.65, 0.8, "impl AppHost");
  wire(s, 5.35, y + h, 5.35, 3.7, { end: false });
  wire(s, 5.35, 3.7, 10.625, 3.7);
  label(s, 5.45, 3.5, 2.5, "AppPort::poll builds one LinkHost");
  // targets
  box(s, 2.55, 4.6, 2.3, 1.0, "IPMI dispatch", ["ipmc-core Context and", "run_request: one local", "request (IPMI)"], W);
  box(s, 5.2, 4.6, 2.3, 1.0, "FRU store", ["ipmc-fru set_unit_field,", "dirty mark, flash log", "(FRU SET, FRU SAVE)"], W);
  box(s, 7.85, 4.6, 2.3, 1.0, "Status and sensors", ["Monitor snapshot, power", "state, NVMRO, uptime", "(INFO, STATUS)"], W);
  box(s, 10.5, 4.6, 2.25, 1.0, "Bootloader request", ["request_bootloader,", "ResetGate, boot request", "words, then reset"], G);
  wire(s, 11.625, 4.1, 11.625, 4.6);
  wire(s, 11.625, 4.35, 3.7, 4.35, { end: false });
  wire(s, 3.7, 4.35, 3.7, 4.6);
  wire(s, 6.35, 4.35, 6.35, 4.6);
  wire(s, 9.0, 4.35, 9.0, 4.6);
  notes(s, 0.6, 5.85, 12.1, 0.95, [
    "One request line at a time: the port takes no new byte while an answer drains. The answer is KEY: value lines and one final line, OK or ERROR <CODE>: <sentence>, with the #tag of the request. The handlers never touch hardware; every board value comes through AppHost.",
    "A custom application adds a row to COMMANDS and, for a board value the trait does not offer, one method of AppHost, implemented in LinkHost and in the test host. tools/app-port/app_cli.py is the host-side reference client.",
  ]);
  legend(s, 0.6, 6.85, [["Firmware code", G], ["ipmc-app and ipmc-core code", W]], 3.0);
  frame(s);
}

pres.writeFile({ fileName: path.join(__dirname, "..", "out", "ipmc-software.pptx") }).then((f) => console.log("wrote", f));
