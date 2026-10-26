/* memory.x - the memory regions of the runtime (cortex-m-rt looks for this file name): the
   flash and RAM map of flash_map.x, the one place of every address, and the stack reservation.
   link.x (this crate's own script) places the sections: the vector table at the start of APP
   (0x0800_C000), the image header at 0x0800_C200, then the program. Nothing of the runtime
   shares BOOT (sectors 0-1, the bootloader) or STORE (sector 2): the FRU log (crates/ipmc-fru
   persist.rs) erases and programs the store, so no code shares it.
   Peers: flash_map.x (the regions), link.x (placement and asserts), src/drivers/flash.rs (the
   store check at start-up). */
INCLUDE flash_map.x

/* Stack reservation: the RAM that must stay free above the statics (.data, .bss) for the
   stack. link.x asserts that RAM from the end of the statics to the top of RAM holds at least
   this much. (.uninit sits in NOINIT, outside RAM, since the relink to 0x0800_C000.) Measured on the release ELF of 10-26-2026 (the default build; the frames do not
   depend on IPMC_LOG_SIZE, the log lives in the static) with arm-none-eabi-objdump -d, every
   function's frame = its push list + vpush + `sub sp`, summed along the longest call chain,
   indirect calls (menu and application port handlers, trait objects) followed by hand:
     start-up   main 9864 + IpmbLink::take 16616 + Link::with_first_request_seq 856 +
                memcpy 72 = 27408 bytes (the largest; the IpmbLink temporary in `take` does not
                shrink when IPMC_LOG_SIZE does)
     run time   main 9864 + menu run_table 752 + deepest console handler (FRU SET, over
                IpmbLink::set_unit_field and with_mfg_date) 2968 = 13584 bytes; the application
                port paths are shorter (FRU SET 608 + 2840, IPMI 688 + 1672 below main)
     interrupts SysTick 120 + I2C event 72 + USART 40 plus three exception frames of 104 bytes
                (FPU context) = 544 bytes on top of either path
   The largest is 27408 + 544 = 27952 bytes. The reservation adds a margin of about 25 percent
   for call chains the graph does not see (fmt through trait objects), rounded up to a whole
   KB: 35 KB (35840 bytes, 28 percent). A change to the measured frames (a new large local in
   `main` or `take`, a larger FRU image or Link) is re-measured and this number changed with
   it. */
_stack_reserve = 35K;
