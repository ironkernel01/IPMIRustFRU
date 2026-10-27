// Drawing set: product-brief (1 sheet: the IPMC and what it connects to) for
// Documentation/ipmc_product_brief.md.
// Build: node diagrams/src/product-brief.js  ->  diagrams/out/product-brief.pptx
// Render: powershell -File diagrams\tools\render.ps1 diagrams\out\product-brief.pptx
// Fills: white = Tier-1 IPMC crates (STM32 and x86), gray = board support (STM32 only),
// blue = Tier-2/3 extension, yellow = outside the IPMC.
const path = require("path");
const { newDeck, helpers, FILL } = require("../lib/engslides");

const pres = newDeck("IronKernel IPMC");
const { box, container, wire, label, notes, legend, frame } = helpers(pres);
const W = FILL.white, G = FILL.gray, B = FILL.blue, Y = FILL.yellow;

const s = pres.addSlide();
const xe = 0.6, ew = 2.2;                 // external column
const c = [3.5, 6.45, 9.4], w = 2.6, h = 0.95;
const r = [1.0, 2.6, 4.2];
const cmx = (i) => c[i] + w / 2, cmy = (j) => r[j] + h / 2;
container(s, 3.2, 0.5, 9.5, 4.95, "IronKernel IPMC: one Arm Cortex-M4F microcontroller");
box(s, xe, r[0], ew, h, "Chassis Manager", ["IPMB-A and IPMB-B", "100 kHz, 400 kHz at Tier-3"], Y);
box(s, xe, r[1], ew, h, "Payload", ["supply rails, power enable,", "power good, fault, reset"], Y);
box(s, xe, r[2], ew, h, "Payload CPU, service PC", ["application port", "console, firmware update"], Y);
box(s, c[0], r[0], w, h, "IPMB drivers", ["two I2C buses, interrupt receive", "hardware or bit-bang master"], G);
box(s, c[1], r[0], w, h, "Link layer", ["frame codec, checksums, retries,", "duplicate cache, bus choice"], W);
box(s, c[2], r[0], w, h, "Command handlers", ["14 Tier-1 commands", "FRU state machine, self test"], W);
box(s, c[0], r[1], w, h, "Board I/O", ["ADC, GPIO, die temperature", "geographic address straps"], G);
box(s, c[1], r[1], w, h, "Sensors and SDR", ["VITA 46.11 sensors", "rails in volts, temperature in C"], W);
box(s, c[2], r[1], w, h, "Tier-2/3 extension", ["events, SEL, activation,", "payload modes, policy bits"], B);
box(s, c[0], r[2], w, h, "Console and app port", ["two UARTs, command line,", "IPMB capture log"], G);
box(s, c[1], r[2], w, h, "Bootloader", ["SHA-256 image check,", "Intel HEX firmware update"], W);
box(s, c[2], r[2], w, h, "FRU inventory", ["512-byte FRU image,", "writes kept in flash"], W);

// Chassis Manager <-> drivers <-> link <-> handlers
wire(s, xe + ew, cmy(0), c[0], cmy(0), { start: true });
label(s, xe + ew + 0.05, cmy(0) - 0.25, 0.65, "IPMB");
wire(s, c[0] + w, cmy(0), c[1], cmy(0), { start: true });
wire(s, c[1] + w, cmy(0), c[2], cmy(0), { start: true });
// handlers -> extension (straight down), handlers -> sensors (down, left, down)
wire(s, cmx(2) + 0.4, r[0] + h, cmx(2) + 0.4, r[1], { start: true });
wire(s, c[2] + 0.3, r[0] + h, c[2] + 0.3, 2.25, { end: false });
wire(s, c[2] + 0.3, 2.25, cmx(1), 2.25, { end: false });
wire(s, cmx(1), 2.25, cmx(1), r[1]);
// handlers -> FRU inventory (right, down, left)
wire(s, c[2] + w, cmy(0), 12.4, cmy(0), { end: false });
wire(s, 12.4, cmy(0), 12.4, cmy(2), { end: false });
wire(s, 12.4, cmy(2), c[2] + w, cmy(2), { start: true });
// payload <-> board I/O -> sensors
wire(s, xe + ew, cmy(1), c[0], cmy(1), { start: true });
wire(s, c[0] + w, cmy(1), c[1], cmy(1));
// serial ports <-> console; console -> bootloader (firmware update)
wire(s, xe + ew, cmy(2), c[0], cmy(2), { start: true });
label(s, xe + ew + 0.05, cmy(2) - 0.25, 0.65, "UART");
wire(s, c[0] + w, cmy(2), c[1], cmy(2), { dash: "dash" });

notes(s, 0.6, 5.6, 12.1, 1.0, [
  "All firmware is Rust: no_std, no heap, no RTOS, no C, no vendor HAL. The IPMI logic forbids unsafe code; only the board support uses unsafe, for register access.",
  "No IPMI code runs in an interrupt: each IPMB interrupt queues whole frames, and the main loop decodes, answers and transmits them. The white blocks also build for x86 as an emulator.",
]);
legend(s, 0.6, 6.8, [["Tier-1 IPMC", W], ["Board support", G], ["Tier-2/3 extension", B], ["Outside the IPMC", Y]], 2.6);
frame(s);

pres.writeFile({ fileName: path.join(__dirname, "..", "out", "product-brief.pptx") });
