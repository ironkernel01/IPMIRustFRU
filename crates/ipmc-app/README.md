# ipmc-app

The application port protocol of the IPMC: what a payload CPU on the application UART (APP_UART,
USART1, PB6 TX and PB7 RX) can ask the IPMC, and how the IPMC answers. One text for a person at a
terminal and a payload CPU alike: the prompt `app> `, an echo of what is typed, then for each
request zero or more `KEY: value` data lines and one final line, `OK` or
`ERROR <CODE>: <sentence>`.

- The format, the parsing rule for a payload CPU, a session and the recipe for a new command are
  the module comment of `src/lib.rs`; read that page first.
- `src/engine.rs` is the command engine (syntax strings `FRU SET [FIELD] {VALUE...}`, matching,
  `HELP`); `src/commands.rs` holds the one command table `COMMANDS` and the handlers; `src/line.rs`
  holds the line editor (echo, backspace, CR LF forms).
- `Documentation/ipmc_software_spec.md` section 7.6 has the command table, the error codes and a
  session, and section 6.8 describes the files of this crate.
- The crate never touches hardware. It asks the board through the `AppHost` trait (`src/host.rs`);
  the firmware implements the trait in `crates/ipmc-nucleo-f401re/src/app_port.rs`, and the tests
  implement it over a plain `FruStore` (`src/tests.rs`).
- The bench client is `tools/app-port/app_cli.py` (see its README).

Tests: `cargo test -p ipmc-app`.
