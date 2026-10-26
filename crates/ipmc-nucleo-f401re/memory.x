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
   this much. Measured on the release ELF at IPMC_LOG_SIZE 16384, the largest the guard in
   ipmc-defs allows (arm-none-eabi-objdump -d, every function's frame = its push list + vpush +
   `sub sp`, summed along the longest call chain, indirect menu-handler calls added by hand):
     start-up   main 8232 + IpmbLink::take 16600 + Link::with_first_request_seq 840 +
                memcpy 72 = 25744 bytes (the largest; the IpmbLink temporary in `take` does not
                shrink when IPMC_LOG_SIZE does, the log lives in the static)
     run time   main 8232 + menu run_table 816 + deepest console handler (FRU RESTORE, over
                Persist::save) 2884 = 11932 bytes
     interrupts SysTick 108 + I2C event 80 + USART 40 plus three exception frames of 104 bytes
                (FPU context) = 540 bytes on top of either path
   The largest is 25744 + 540 = 26284 bytes. The reservation adds a margin of about 25 percent
   for call chains the graph does not see (fmt through trait objects), rounded up to a whole
   KB: 32 KB. A change to the measured frames (a new large local in `main` or `take`, a larger
   FRU image or Link) is re-measured and this number changed with it. */
_stack_reserve = 32K;

__fru_store_start = ORIGIN(STORE);
__fru_store_end = ORIGIN(STORE) + LENGTH(STORE);
