# ipmc-app

The application port protocol of the IPMC: what a payload CPU on the application UART (APP_UART,
USART1, PB6 TX and PB7 RX) can ask the IPMC, and how the IPMC answers. One ASCII request line
gets one answer line, `OK ...` or `ERR <CODE> <text>`.

- The protocol, the line format, the error codes and the recipe for a new command are the module
  comment of `src/lib.rs`; read that page first.
- `Documentation/ipmc_software_spec.md` section 7.6 has the command table and a worked exchange,
  and section 6.8 describes the files of this crate.
- The crate never touches hardware. It asks the board through the `AppHost` trait (`src/host.rs`);
  the firmware implements the trait in `crates/ipmc-nucleo-f401re/src/app_port.rs`, and the tests
  implement it over a plain `FruStore` (`src/tests.rs`).
- The bench client is `tools/app-port/app_cli.py` (see its README).

Tests: `cargo test -p ipmc-app`.
