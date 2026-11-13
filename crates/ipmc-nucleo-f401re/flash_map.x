/* flash_map.x - the one place of every flash and RAM address of the NUCLEO-F401RE images
   (local_only/Documentation/ipmc_boot_plan.md section 2).

   Both linker scripts include it: memory.x (the runtime, through link.x) and boot.x (the
   bootloader in sectors 0-1). The numbers are RM0368 Table 5 (STM32F401xE, 512 KB: sectors 0-3 are
   16 KB, sector 4 is 64 KB, sectors 5-7 are 128 KB) and the owner's layout:

     BOOT    sectors 0-1  0x0800_0000..0x0800_7FFF   32 KB   the bootloader (SWD or the drive copy)
     STORE   sector 2     0x0800_8000..0x0800_BFFF   16 KB   the FRU flash store, nothing else
     APP     sectors 3-7  0x0800_C000..0x0807_FFFF  464 KB   the runtime: vector table at the
                                                             start, the image header at +0x200
     NOINIT  RAM          0x2000_0000..0x2000_00FF  256 B    the boot request (offset 0), the
                                                             runtime's .uninit (offset 0x10) and the
                                                             kept fault record (offset 0xF0, the last
                                                             16 bytes)
     RAM     RAM          0x2000_0100..0x2001_7FFF  95.75 KB .data, .bss and the stack of
                                                             whichever image runs

   Peers (the same numbers): crates/ipmc-boot/src/geometry.rs (Rust constants; the test
   crates/ipmc-boot/tests/flash_map.rs parses this file and fails on a difference),
   crates/ipmc-fru/src/persist.rs (STORE_START, STORE_LEN; drivers/flash.rs checks the linked
   store region against them at start-up), src/bsp/boot_request.rs (reads __boot_request),
   src/bsp/fault_record.rs (reads and writes __fault_record).
   Drift failure mode: a bootloader and a runtime built from different versions of this file
   disagree on the header or the request address; the bootloader then finds no header and stays
   (safe), and a request is lost (the board starts the runtime). The asserts at the end of this
   file check the order of the regions. */
MEMORY
{
  BOOT   : ORIGIN = 0x08000000, LENGTH = 32K    /* sectors 0-1: the bootloader */
  STORE  : ORIGIN = 0x08008000, LENGTH = 16K    /* sector 2: the FRU flash store */
  APP    : ORIGIN = 0x0800C000, LENGTH = 464K   /* sectors 3-7: the runtime */
  NOINIT : ORIGIN = 0x20000000, LENGTH = 256    /* boot request (0), runtime .uninit (0x10), fault record (0xF0) */
  RAM    : ORIGIN = 0x20000100, LENGTH = 96K - 256
}

__boot_end = ORIGIN(BOOT) + LENGTH(BOOT);
__fru_store_start = ORIGIN(STORE);
__fru_store_end = ORIGIN(STORE) + LENGTH(STORE);
__app_vectors = ORIGIN(APP);
__app_header = ORIGIN(APP) + 0x200;
__boot_request = ORIGIN(NOINIT);
__fault_record = ORIGIN(NOINIT) + 0xF0;

ASSERT(ORIGIN(BOOT) + LENGTH(BOOT) == ORIGIN(STORE), "
ERROR(ipmc): flash_map.x: STORE does not follow BOOT");

ASSERT(ORIGIN(STORE) + LENGTH(STORE) == ORIGIN(APP), "
ERROR(ipmc): flash_map.x: APP does not follow STORE");

ASSERT(ORIGIN(NOINIT) + LENGTH(NOINIT) == ORIGIN(RAM), "
ERROR(ipmc): flash_map.x: RAM does not follow NOINIT");
