// Drawing set: ipmc-software (4 sheets) for Documentation/ipmc_software_spec.md.
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
  container(s, 0.6, 1.3, 3.15, 2.55, "Firmware workspace");
  container(s, 4.2, 0.5, 8.55, 5.1, "Root cargo workspace");
  const c1 = 4.5, c2 = 9.1, w = 3.3;
  // application port path: payload CPU -> APP_UART -> bsp/uart.rs -> app_port.rs -> ipmc-app
  box(s, 0.9, 0.5, 2.55, 0.38, "Payload CPU");
  wire(s, 2.175, 0.88, 2.175, 1.6);
  label(s, 2.275, 0.92, 1.4, "APP_UART, USART1");
  label(s, 2.275, 1.08, 1.4, "PB6 TX / PB7 RX");
  box(s, 0.9, 1.6, 2.55, 0.55, "bsp/uart.rs", ["one UART driver, RX/TX rings", "APP_UART and console (USART2)"], G);
  wire(s, 2.175, 2.15, 2.175, 2.35, { start: true });
  box(s, 0.9, 2.35, 2.55, 0.55, "app_port.rs", ["AppPort::poll, LinkHost", "implements AppHost, IPMB link"], G);
  box(s, 0.9, 3.1, 2.55, 0.55, "ipmc-nucleo-f401re", ["STM32F401RE firmware", "main loop, BSP drivers, console"], G);
  box(s, c2, 0.85, w, 0.6, "ipmc-emu", ["x86 emulator", "replay, serve, sdr"], B);
  box(s, c2, 1.75, w, 0.6, "ipmc-sensors", ["sensor and board I/O HAL", "hal, eval, drivers, board table"], W);
  box(s, c1, 0.85, w, 0.6, "ipmc-app", ["application port protocol, no_std", "one request line, one OK/ERR line"], W);
  box(s, c1, 1.75, w, 0.6, "ipmc-menu", ["console menu engine", "MenuBuilder rules"], W);
  box(s, c2, 2.65, w, 0.85, "ipmc-core", ["all IPMI logic", "codec, link layer, commands, SDR,", "sensors, FRU state, self test"], W);
  box(s, c2, 3.8, w, 0.6, "ipmc-fru", ["FRU records and scratchpad", "image, store, flash log"], W);
  box(s, c2, 4.7, w, 0.6, "ipmc-wire", ["IPMI byte primitives", "checksum, builder, type/length, CRC-32"], W);
  box(s, c1, 4.7, w, 0.6, "ipmc-defs", ["typed constants (defs.rs)", "and compile-time guards"], W);
  box(s, c1, 6.1, w, 0.6, "defs.rs", ["build configuration", "Rust pub const items"], Y);
  box(s, 6.5, 2.95, 2.2, 0.75, "ipmc-ext", ["bridge crate: Active is", "NoExtension or the", "extension, by ipmc-defs"], W);
  box(s, 8.2, 6.1, 4.2, 0.6, "Tier-2/3 extension", ["(commercial release build, private)", "sibling folder, mounted by ipmc-ext"], W);
  const x2 = cx(c2, w), x1 = cx(c1, w);
  wire(s, x2, 1.45, x2, 1.75);
  wire(s, x2, 2.35, x2, 2.65);
  wire(s, x2, 3.5, x2, 3.8);
  wire(s, x2, 4.4, x2, 4.7);
  // app_port.rs <-> ipmc-app (L route, two-way); firmware -> ipmc-menu (L route)
  wire(s, 3.45, 2.625, 3.95, 2.625, { start: true, end: false });
  wire(s, 3.95, 2.625, 3.95, 1.15, { end: false });
  wire(s, 3.95, 1.15, c1, 1.15);
  wire(s, 3.45, 3.375, 4.1, 3.375, { end: false });
  wire(s, 4.1, 3.375, 4.1, 2.05, { end: false });
  wire(s, 4.1, 2.05, c1, 2.05);
  // ipmc-app uses ipmc-core
  wire(s, c1 + w, 1.15, 8.45, 1.15, { end: false });
  wire(s, 8.45, 1.15, 8.45, 2.8, { end: false });
  wire(s, 8.45, 2.8, c2, 2.8);
  wire(s, x1, 2.35, x1, 4.7);
  wire(s, x1, 6.1, x1, 5.3, { dash: "dash" });
  label(s, x1 + 0.1, 5.75, 1.5, "lib.rs: mod defs");
  wire(s, 8.7, 3.3, c2, 3.3);
  wire(s, 7.2, 3.7, 7.2, 4.7);
  wire(s, 8.4, 3.7, 8.4, 6.1, { dash: "dash" });
  label(s, 8.5, 5.75, 1.6, "#[path] mount, when found");
  notes(s, 0.6, 3.95, 3.5, 2.8, [
    "Every crate reads defs.rs values as ipmc_defs::NAME; those arrows are not drawn.",
    "ipmc-ext uses ipmc-core and ipmc-defs; ipmc-nucleo-f401re and ipmc-emu use ipmc-ext (arrows not drawn). The ipmc-defs build.rs finds the extension folder; without it ipmc-ext holds NoExtension and the build is Tier-1.",
    "ipmc-nucleo-f401re uses ipmc-core, -fru, -app, -menu, -sensors (feature stm32f401) and -defs. ipmc-emu uses core, fru, sensors, wire and defs. ipmc-app uses core, fru, wire and defs. ipmc-sensors uses core and defs. ipmc-core uses fru, wire and defs. ipmc-fru uses wire and defs. ipmc-menu uses defs. Arrows that would cross are not drawn.",
    "The firmware crate builds on its own (thumbv7em-none-eabihf). ipmc-core, ipmc-fru, ipmc-wire, ipmc-defs, ipmc-menu and ipmc-app are no_std with forbid(unsafe_code); ipmc-sensors allows unsafe only in drivers/stm32f401.",
    "ipmc-app touches no hardware; app_port.rs implements its AppHost trait over the IPMB link, which reaches the ipmc-core dispatcher and the ipmc-fru store. The console and APP_UART share bsp/uart.rs. The payload CPU is board hardware. ipmc-emu does not build ipmc-app.",
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
    "The fault tracker (ipmb_link.rs) and the FRU flash log save run in the same pass and are on sheet 3.",
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
  box(s, 10.2, L, 2.2, h, "Flash log", ["bsp/flash.rs, sectors 6 and 7", "0x0804_0000 and 0x0806_0000"], G);
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
  ]);
  legend(s, 0.6, 6.85, LEG.slice(1, 3), 3.0);
  frame(s);
}

pres.writeFile({ fileName: path.join(__dirname, "..", "out", "ipmc-software.pptx") }).then((f) => console.log("wrote", f));
