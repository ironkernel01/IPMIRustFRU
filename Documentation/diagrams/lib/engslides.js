// engslides.js: engineering block-diagram helpers for pptxgenjs.
// One drawing per slide, LAYOUT_WIDE (13.33 x 7.5 in), coordinates in inches.
const pptxgen = require("pptxgenjs");

const FONT = "Arial";
const BLACK = "000000";
// Classification fills. A drawing that uses them defines their meaning in a legend
// on the same sheet (for example white = reuse as-is, gray = modified, yellow = new).
const FILL = { white: "FFFFFF", gray: "E7E6E6", yellow: "FFF2CC", blue: "DDEBF7" };

function newDeck(title, author) {
  const pres = new pptxgen();
  pres.layout = "LAYOUT_WIDE";
  pres.title = title;
  if (author) pres.author = author;
  return pres;
}

function helpers(pres) {
  // Block: bold title line plus optional detail lines, black outline.
  function box(s, x, y, w, h, title, lines = [], fill = FILL.white) {
    const runs = [{ text: title, options: { bold: true, fontSize: 9.5, breakLine: lines.length > 0 } }];
    lines.forEach((t, i) => runs.push({ text: t, options: { fontSize: 8, breakLine: i < lines.length - 1 } }));
    s.addText(runs, {
      shape: pres.shapes.RECTANGLE, x, y, w, h,
      fill: { color: fill }, line: { color: BLACK, width: 0.75 },
      fontFace: FONT, color: BLACK, align: "center", valign: "middle",
      margin: 2, objectName: title,
    });
  }

  // Boundary (chip, board, software layer): dashed outline, label top-left inside.
  function container(s, x, y, w, h, label) {
    s.addShape(pres.shapes.RECTANGLE, {
      x, y, w, h, fill: { type: "none" },
      line: { color: BLACK, width: 0.75, dashType: "dash" },
    });
    s.addText(label, {
      x: x + 0.08, y: y + 0.04, w: 4, h: 0.22, isTextBox: true,
      fontFace: FONT, fontSize: 9, bold: true, color: BLACK, margin: 0, valign: "top",
    });
  }

  // Straight horizontal or vertical connector from (x1,y1) to (x2,y2).
  // end: arrowhead at (x2,y2); start: arrowhead at (x1,y1); dash: "dash" for control/config paths.
  // Route an L-shaped path as two wire() calls.
  function wire(s, x1, y1, x2, y2, { start = false, end = true, dash } = {}) {
    const opts = {
      x: Math.min(x1, x2), y: Math.min(y1, y2),
      w: Math.abs(x2 - x1), h: Math.abs(y2 - y1),
      line: { color: BLACK, width: 0.75 },
    };
    if (dash) opts.line.dashType = dash;
    if (start) opts.line.beginArrowType = "triangle";
    if (end) opts.line.endArrowType = "triangle";
    if (x2 < x1) opts.flipH = true;
    if (y2 < y1) opts.flipV = true;
    s.addShape(pres.shapes.LINE, opts);
  }

  // Signal or connector label; place it 0.2 in above a horizontal wire.
  function label(s, x, y, w, text, size = 7.5, align = "left") {
    s.addText(text, {
      x, y, w, h: 0.18, isTextBox: true, fontFace: FONT, fontSize: size,
      color: BLACK, margin: 0, align, valign: "bottom",
    });
  }

  // Numbered notes block.
  function notes(s, x, y, w, h, items) {
    const runs = [{ text: "NOTES:", options: { bold: true, breakLine: true } }];
    items.forEach((t, i) => runs.push({ text: `${i + 1}. ${t}`, options: { breakLine: i < items.length - 1 } }));
    s.addText(runs, {
      x, y, w, h, isTextBox: true, fontFace: FONT, fontSize: 7.5, color: BLACK,
      margin: 0, valign: "top", paraSpaceAfter: 2,
    });
  }

  // Legend row for classification fills: items = [["Reuse as-is", FILL.white], ...]
  function legend(s, x, y, items, step = 2.3) {
    items.forEach(([t, f], i) => {
      const lx = x + i * step;
      s.addShape(pres.shapes.RECTANGLE, { x: lx, y, w: 0.3, h: 0.18, fill: { color: f }, line: { color: BLACK, width: 0.75 } });
      label(s, lx + 0.38, y, step - 0.4, t, 8);
    });
  }

  // Sheet border. Keep drawing content inside x 0.6 to 12.75 in and y 0.4 to 7.1 in.
  function frame(s) {
    s.addShape(pres.shapes.RECTANGLE, {
      x: 0.25, y: 0.25, w: 12.83, h: 7.0, fill: { type: "none" },
      line: { color: BLACK, width: 1.5 },
    });
  }

  return { box, container, wire, label, notes, legend, frame };
}

module.exports = { newDeck, helpers, FILL, FONT, BLACK };
