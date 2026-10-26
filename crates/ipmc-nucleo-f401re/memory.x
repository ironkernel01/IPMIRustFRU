/* Flash layout of the STM32F401RE (RM0368 Table 5: sectors 0-3 are 16 KB, sector 4 is 64 KB,
   sectors 5-7 are 128 KB; 512 KB from 0x0800_0000).
     VECTORS  sector 0    0x0800_0000..0x0800_3FFF   16 KB   .vector_table only
     STORE    sector 1    0x0800_4000..0x0800_7FFF   16 KB   the FRU flash store, nothing else
     FLASH    sectors 2-7 0x0800_8000..0x0807_FFFF  480 KB   .text, .rodata, the .data load image
   link.x (this crate's own script) places the sections. The store is the one sector the FRU log
   (crates/ipmc-fru persist.rs) erases and programs: no code shares it, so an erase never touches
   the vector table or the program.
   Peers: link.x (placement and asserts); src/bsp/flash.rs, which reads __fru_store_start and
   __fru_store_end at run time and refuses to start the store unless the region is exactly
   sector 1 (ipmc_fru::persist::store_region_ok); ipmc-fru STORE_START and STORE_LEN, the same
   numbers. The linker fails the build when a section no longer fits its region. */
MEMORY
{
  VECTORS : ORIGIN = 0x08000000, LENGTH = 16K
  STORE   : ORIGIN = 0x08004000, LENGTH = 16K
  FLASH   : ORIGIN = 0x08008000, LENGTH = 480K
  RAM     : ORIGIN = 0x20000000, LENGTH = 96K
}

/* Stack reservation: the RAM that must stay free above the statics (.data, .bss, .uninit) for the
   stack. link.x asserts that RAM from the end of the statics to the top of RAM holds at least
   this much. Measured on the release ELF of 10-26-2026 (the default build; the frames do not
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

__fru_store_start = ORIGIN(STORE);
__fru_store_end = ORIGIN(STORE) + LENGTH(STORE);
