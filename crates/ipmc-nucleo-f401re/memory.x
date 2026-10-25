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

__fru_store_start = ORIGIN(STORE);
__fru_store_end = ORIGIN(STORE) + LENGTH(STORE);
