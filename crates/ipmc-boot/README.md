# ipmc-boot

The library of the bootloader (sectors 0-1) (local_only/Documentation/ipmc_boot_plan.md): the flash geometry and
region policy, the runtime image header and its check, the boot request words, the start decision
and countdown, the start-up trace text, the Intel HEX parser and the `LOAD` state machine. It is
`no_std`, `forbid(unsafe_code)`, panic free and host-tested; it uses no `core::fmt`.

The crate has no firmware binary. The bootloader binary `ipmc-boot` is built from the F401 crate
`crates/ipmc-nucleo-f401re` (`src/boot/`, linker script `boot.x`, profile `boot`), because every
STM32F401 specific file lives there:

```
cd crates/ipmc-nucleo-f401re
cargo build --profile boot --bin ipmc-boot          # the bootloader
cargo build --release --bin ipmc-nucleo-f401re      # the runtime
cargo clippy --profile boot --bin ipmc-boot -- -D warnings
```

`gen.ps1 build` runs both builds and the image steps (header patch, check, hex, board image).

`fwimage` (`src/bin/fwimage.rs`, host) patches the header into a built runtime image, checks it,
combines the board image and checks the bootloader size; `gen.ps1` runs it with
`cargo run -q -p ipmc-boot --bin fwimage --`.

`tests/flash_map.rs` parses `crates/ipmc-nucleo-f401re/flash_map.x` (the linker's one place of
every address) and fails when it differs from `geometry.rs`.
