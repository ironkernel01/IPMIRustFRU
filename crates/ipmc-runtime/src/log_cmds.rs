// log_cmds.rs - the console handlers of the logging half of the DEBUG group: LOG, DUMP, CLEAR and
// TRACE.
//
// Each handler parses its words with the ipmc-menu helpers, calls ipmc-log (the capture log, the
// dump job, the trace switches) and prints the Display lines that crate builds. The state lives in
// `IpmbLink::log` (ipmc_runtime::link); the console is the `DumpSink` of the paced dump. Nothing here
// runs in an interrupt handler.

use ipmc_log::{DumpFormat, DumpHeader, DumpJob, LogLine, SwitchLines, TraceWhat};
use ipmc_menu::{optional, Args, CmdResult};

use ipmc_hal::board::Board;
use ipmc_hal::time::Clock;

use crate::console::{Context, LinkHome};
use crate::debug_cmds::reported;
use crate::out;

/// cmd_debug_log -- DEBUG LOG {STATE}.
pub fn cmd_debug_log<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    if let Some(on) = optional(args.on_off_at(0))? {
        ctx.ipmb.log_mut().capture_mut().set_enabled(on);
    }
    out!(con, "{}", LogLine(ctx.ipmb.log()));
    Ok(())
}

/// cmd_debug_dump -- DEBUG DUMP {FORMAT}: prints the header line and starts the dump job; the
/// console prints the rows and the end line on the following main-loop passes (`Console::poll`).
pub fn cmd_debug_dump<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let format = match optional(args.text_at(0))? {
        None => DumpFormat::Csv,
        Some(word) => DumpFormat::parse(word)
            .ok_or_else(|| reported(con, format_args!("format must be CSV or TRACE")))?,
    };
    let link = &*ctx.ipmb;
    out!(
        con,
        "{}\n",
        DumpHeader {
            firmware: ipmc_defs::IPMC_FW_NAME,
            version: ctx.start.version,
            core_version: ipmc_core::CORE_VERSION,
            ipmb_address: link.dispatch().addr.ipmb_address,
            tier: ipmc_defs::IPMC_TIER,
            uptime_ms: ctx.clock.millis(),
            stats: link.log().capture().stats(),
        }
    );
    if let Some(columns) = format.column_header() {
        out!(con, "{}\n", columns);
    }
    ctx.dump = Some(DumpJob::start(format, link.log().capture()));
    Ok(())
}

/// cmd_debug_clear -- DEBUG CLEAR.
pub fn cmd_debug_clear<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    ctx.ipmb.log_mut().capture_mut().clear();
    out!(con, "OK: capture log cleared\n");
    Ok(())
}

/// cmd_debug_trace -- DEBUG TRACE {WHAT} {STATE}: IPMI (the request, link and event lines on
/// the console; the log keeps them either way) or I2C (one console line per frame).
pub fn cmd_debug_trace<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    if let Some(word) = optional(args.text_at(0))? {
        let state = optional(args.on_off_at(1))?;
        let what = TraceWhat::parse(word)
            .ok_or_else(|| reported(con, format_args!("trace is IPMI or I2C")))?;
        if let Some(on) = state {
            ctx.ipmb.log_mut().set_trace(what, on);
        }
    }
    out!(con, "{}", SwitchLines(ctx.ipmb.log()));
    Ok(())
}
