/* FLASH holds the program: sectors 0-5 of the STM32F401RE (RM0368 Table 5, 256 KB).
   LOG is sectors 6 and 7, 128 KB each: the FRU log (crates/ipmc-fru persist.rs).
   Peer: crates/ipmc-nucleo-f401re/src/bsp/flash.rs (LOG_SECTOR_ADDR). If FLASH grows past
   0x08040000 the program overwrites the log; bsp::flash::Flash::new refuses to start when the
   linked image reaches into it. The linker fails the build when the image no longer fits LENGTH. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 256K
  LOG   : ORIGIN = 0x08040000, LENGTH = 256K
  RAM   : ORIGIN = 0x20000000, LENGTH = 96K
}
