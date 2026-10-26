// log_cmds.rs - the console handlers of the logging half of the DEBUG group: LOG, DUMP, CLEAR and
// TRACE.
//
// Each handler parses its words with the ipmc-menu helpers, calls ipmc-log (the capture log, the
// dump job, the trace switches) and prints the Display lines that crate builds. The state lives in
// `IpmbLink::log` (ipmb_link.rs); the console is the `DumpSink` of the paced dump. Nothing here
// runs in an interrupt handler.

use core::fmt::Arguments;

use ipmc_log::{DumpFormat, DumpHeader, DumpJob, DumpSink, LogLine, SwitchLines, TraceWhat};
use ipmc_menu::{optional, Args, CmdResult};

use crate::bsp::systick;
use crate::bsp::uart::{self, Port};
use crate::console::Context;
use crate::debug_cmds::reported;

/// ConsoleSink -- the console TX ring as the sink of a dump.
pub(crate) struct ConsoleSink;

impl DumpSink for ConsoleSink {
    fn free(&self) -> usize {
        uart::tx_free(Port::Console)
    }

    fn line(&mut self, args: Arguments) {
        uart::print(args);
    }
}

/// cmd_debug_log -- DEBUG LOG {STATE}.
pub fn cmd_debug_log(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    if let Some(on) = optional(args.on_off_at(0))? {
        ctx.ipmb.log_mut().capture_mut().set_enabled(on);
    }
    out!("{}", LogLine(ctx.ipmb.log()));
    Ok(())
}

/// cmd_debug_dump -- DEBUG DUMP {FORMAT}: prints the header line and starts the dump job; the
/// console prints the rows and the end line on the following main-loop passes (`Console::poll`).
pub fn cmd_debug_dump(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let format = match optional(args.text_at(0))? {
        None => DumpFormat::Csv,
        Some(word) => DumpFormat::parse(word)
            .ok_or_else(|| reported(format_args!("format must be CSV or TRACE")))?,
    };
    let link = &*ctx.ipmb;
    out!(
        "{}\n",
        DumpHeader {
            firmware: ipmc_defs::IPMC_FW_NAME,
            version: env!("CARGO_PKG_VERSION"),
            core_version: ipmc_core::CORE_VERSION,
            ipmb_address: link.dispatch().addr.ipmb_address,
            tier: ipmc_defs::IPMC_TIER,
            uptime_ms: systick::millis(),
            stats: link.log().capture().stats(),
        }
    );
    if let Some(columns) = format.column_header() {
        out!("{}\n", columns);
    }
    ctx.dump = Some(DumpJob::start(format, link.log().capture()));
    Ok(())
}

/// cmd_debug_clear -- DEBUG CLEAR.
pub fn cmd_debug_clear(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    ctx.ipmb.log_mut().capture_mut().clear();
    out!("OK: capture log cleared\n");
    Ok(())
}

/// cmd_debug_trace -- DEBUG TRACE {WHAT} {STATE}: IPMI (the request, link and event lines on
/// the console; the log keeps them either way) or I2C (one console line per frame).
pub fn cmd_debug_trace(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    if let Some(word) = optional(args.text_at(0))? {
        let state = optional(args.on_off_at(1))?;
        let what =
            TraceWhat::parse(word).ok_or_else(|| reported(format_args!("trace is IPMI or I2C")))?;
        if let Some(on) = state {
            ctx.ipmb.log_mut().set_trace(what, on);
        }
    }
    out!("{}", SwitchLines(ctx.ipmb.log()));
    Ok(())
}
