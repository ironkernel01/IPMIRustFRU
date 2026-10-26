/* link.x - the linker script of ipmc-nucleo-f401re.

   Derived from link.x.in of cortex-m-rt 0.7.7 (https://github.com/rust-embedded/cortex-m,
   licensed MIT OR Apache-2.0, Copyright the Cortex-M Developers; the developer notes and the
   checks of that file are kept). This is the script of the runtime firmware; the bootloader
   (sectors 0-1) has boot.x. Both include flash_map.x, the one place of every address. Layout:

   - The whole image sits in APP (flash sectors 3-7, from 0x0800_C000): .vector_table at
     ORIGIN(APP), then the 64-byte .app_header at __app_header (ORIGIN(APP) + 0x200, the
     ipmc_boot::header image header), then .text, .rodata, the .data load image and veneers.
   - BOOT (sectors 0-1) is written by the bench only (SWD or the drive copy: the bootloader). STORE
     (sector 2) receives no output section: the FRU flash store (crates/ipmc-fru persist.rs)
     erases and programs it at run time. Sectors 3-7 are written by the bootloader's LOAD, by SWD
     or by the drive copy. The runtime writes sector 2 only.
   - .uninit (NOLOAD) sits in NOINIT at ORIGIN(NOINIT) + 0x10; the boot request words are the
     first 16 bytes (__boot_request), written by src/bsp/boot_request.rs and read by the
     bootloader, so no other section may use them.
   - Symbols: __program_start (start of .text) and __sidata/__sdata/__edata tell
     drivers/flash.rs where the program ends; flash_map.x exports __app_vectors, __app_header,
     __boot_end, __fru_store_start and __fru_store_end.
   - Added asserts: the vector table, the header and the program sit at their addresses and
     inside APP (nothing in BOOT or STORE), the header is 64 bytes, and .uninit fits NOINIT above
     the request words.

   Peer: memory.x (INCLUDE flash_map.x) and src/drivers/flash.rs (the store check at start-up),
   src/app_header.rs (the header static). The link of device.x (the interrupt handler defaults
   of the device crate) is at the end, as cortex-m-rt appends it for a device crate. */

/* # Developer notes

- Symbols that start with a double underscore (__) are considered "private"

- Symbols that start with a single underscore (_) are considered "semi-public"; they can be
  overridden in a user linker script, but should not be referred from user code (e.g. `extern "C" {
  static mut __sbss }`).

- `EXTERN` forces the linker to keep a symbol in the final binary. We use this to make sure a
  symbol is not dropped if it appears in or near the front of the linker arguments and "it's not
  needed" by any of the preceding objects (linker arguments)

- `PROVIDE` is used to provide default values that can be overridden by a user linker script

- On alignment: it's important for correctness that the VMA boundaries of both .bss and .data *and*
  the LMA of .data are all 4-byte aligned. These alignments are assumed by the RAM initialization
  routine. There's also a second benefit: 4-byte aligned boundaries means that you won't see
  "Address (..) is out of bounds" in the disassembly produced by `objdump`.
*/

/* Provides information about the memory layout of the device */
/* This will be provided by the user (see `memory.x`) or by a Board Support Crate */
INCLUDE memory.x

/* # Entry point = reset vector */
EXTERN(__RESET_VECTOR);
EXTERN(Reset);
ENTRY(Reset);

/* # Exception vectors */
/* This is effectively weak aliasing at the linker level */
/* The user can override any of these aliases by defining the corresponding symbol themselves (cf.
   the `exception!` macro) */
EXTERN(__EXCEPTIONS); /* depends on all the these PROVIDED symbols */

EXTERN(DefaultHandler);

PROVIDE(NonMaskableInt = DefaultHandler);
EXTERN(HardFaultTrampoline);
PROVIDE(MemoryManagement = DefaultHandler);
PROVIDE(BusFault = DefaultHandler);
PROVIDE(UsageFault = DefaultHandler);
PROVIDE(SecureFault = DefaultHandler);
PROVIDE(SVCall = DefaultHandler);
PROVIDE(DebugMonitor = DefaultHandler);
PROVIDE(PendSV = DefaultHandler);
PROVIDE(SysTick = DefaultHandler);

PROVIDE(DefaultHandler = DefaultHandler_);
PROVIDE(HardFault = HardFault_);

/* # Interrupt vectors */
EXTERN(__INTERRUPTS); /* `static` variable similar to `__EXCEPTIONS` */

/* # Pre-initialization function */
/* If the user overrides this using the `pre_init!` macro or by creating a `__pre_init` function,
   then the function this points to will be called before the RAM is initialized. */
PROVIDE(__pre_init = DefaultPreInit);

/* # Sections */
SECTIONS
{
  PROVIDE(_ram_start = ORIGIN(RAM));
  PROVIDE(_ram_end = ORIGIN(RAM) + LENGTH(RAM));
  PROVIDE(_stack_start = _ram_end);

  /* ## Sections in FLASH */
  /* ### Vector table */
  .vector_table __app_vectors :
  {
    __vector_table = .;

    /* Initial Stack Pointer (SP) value.
     * We mask the bottom three bits to force 8-byte alignment.
     * Despite having an assert for this later, it's possible that a separate
     * linker script could override _stack_start after the assert is checked.
     */
    LONG(_stack_start & 0xFFFFFFF8);

    /* Reset vector */
    KEEP(*(.vector_table.reset_vector)); /* this is the `__RESET_VECTOR` symbol */

    /* Exceptions */
    __exceptions = .; /* start of exceptions */
    KEEP(*(.vector_table.exceptions)); /* this is the `__EXCEPTIONS` symbol */
    __eexceptions = .; /* end of exceptions */

    /* Device specific interrupts */
    KEEP(*(.vector_table.interrupts)); /* this is the `__INTERRUPTS` symbol */
  } > APP

  /* ### Image header (ipmc_boot::header, 64 bytes at ORIGIN(APP) + 0x200) */
  .app_header __app_header :
  {
    KEEP(*(.app_header));
  } > APP

  /* The alignment of .text is the maximum alignment required by its input sections, which
     can be more than 4 (e.g. `.p2align` directives inside inline assembly). Since the
     start address of .text is set explicitly, the linker won't align it automatically, so do it
     here. The program starts after the header. */
  PROVIDE(_stext = ALIGN(ADDR(.app_header) + SIZEOF(.app_header), ALIGNOF(.text)));

  /* ### .text */
  .text _stext :
  {
    __stext = .;
    __program_start = .;
    *(.Reset);

    *(.text .text.*);

    /* The HardFaultTrampoline uses the `b` instruction to enter `HardFault`,
       so must be placed close to it. */
    *(.HardFaultTrampoline);
    *(.HardFault.*);

    . = ALIGN(4); /* Pad .text to the alignment to workaround overlapping load section bug in old lld */
    __etext = .;
  } > APP

  /* ### .rodata */
  .rodata : ALIGN(4)
  {
    . = ALIGN(4);
    __srodata = .;
    *(.rodata .rodata.*);

    /* 4-byte align the end (VMA) of this section.
       This is required by LLD to ensure the LMA of the following .data
       section will have the correct alignment. */
    . = ALIGN(4);
    __erodata = .;
  } > APP

  /* ## Sections in RAM */
  /* ### .data */
  .data : ALIGN(4)
  {
    . = ALIGN(4);
    __sdata = .;
    *(.data .data.*);
    . = ALIGN(4); /* 4-byte align the end (VMA) of this section */
  } > RAM AT>APP
  /* Allow sections from user `memory.x` injected using `INSERT AFTER .data` to
   * use the .data loading mechanism by pushing __edata. Note: do not change
   * output region or load region in those user sections! */
  . = ALIGN(4);
  __edata = .;

  /* LMA of .data */
  __sidata = LOADADDR(.data);

  /* ### .gnu.sgstubs
     This section contains the TrustZone-M veneers put there by the Arm GNU linker. */
  /* Security Attribution Unit blocks must be 32 bytes aligned. */
  /* Note that this pads the FLASH usage to 32 byte alignment. */
  .gnu.sgstubs : ALIGN(32)
  {
    . = ALIGN(32);
    __veneer_base = .;
    *(.gnu.sgstubs*)
    . = ALIGN(32);
  } > APP
  /* Place `__veneer_limit` outside the `.gnu.sgstubs` section because veneers are
   * always inserted last in the section, which would otherwise be _after_ the `__veneer_limit` symbol.
   */
  . = ALIGN(32);
  __veneer_limit = .;

  /* ### .bss */
  .bss (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    __sbss = .;
    *(.bss .bss.*);
    *(COMMON); /* Uninitialized C statics */
    . = ALIGN(4); /* 4-byte align the end (VMA) of this section */
  } > RAM
  /* Allow sections from user `memory.x` injected using `INSERT AFTER .bss` to
   * use the .bss zeroing mechanism by pushing __ebss. Note: do not change
   * output region or load region in those user sections! */
  . = ALIGN(4);
  __ebss = .;

  /* ### .uninit: in NOINIT, after the 16 bytes of the boot request (__boot_request) */
  .uninit (ORIGIN(NOINIT) + 0x10) (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    __suninit = .;
    *(.uninit .uninit.*);
    . = ALIGN(4);
    __euninit = .;
  } > NOINIT

  /* Place the heap start and stack end at the end of allocated RAM (.bss; .uninit is in
     NOINIT, not in RAM). The location counter is in NOINIT here, so the end of .bss is used. */
  PROVIDE(__sheap = __ebss);
  PROVIDE(_stack_end = __ebss);

  /* ## .got */
  /* Dynamic relocations are unsupported. This section is only used to detect relocatable code in
     the input files and raise an error if relocatable code is found */
  .got (NOLOAD) :
  {
    KEEP(*(.got .got.*));
  }

  /* ## Discarded sections */
  /DISCARD/ :
  {
    /* Unused exception related info that only wastes space */
    *(.ARM.exidx);
    *(.ARM.exidx.*);
    *(.ARM.extab.*);
  }
}

/* Do not exceed this mark in the error messages below                                    | */
/* # Alignment checks */
ASSERT(ORIGIN(APP) % 4 == 0, "
ERROR(cortex-m-rt): the start of the APP region must be 4-byte aligned");

/* # Image placement checks (not in cortex-m-rt) */
ASSERT(ADDR(.vector_table) == __app_vectors, "
ERROR(ipmc): the vector table is not at the start of APP (0x0800_C000)");

ASSERT(ADDR(.app_header) == __app_header, "
ERROR(ipmc): the image header is not at ORIGIN(APP) + 0x200 (0x0800_C200)");

ASSERT(SIZEOF(.app_header) == 64, "
ERROR(ipmc): the .app_header section is not 64 bytes (src/app_header.rs, ipmc_boot::header)");

ASSERT(ADDR(.vector_table) + SIZEOF(.vector_table) <= __app_header, "
ERROR(ipmc): the vector table reaches into the image header");

ASSERT(__program_start >= __app_header + 64, "
ERROR(ipmc): the program starts inside the image header");

ASSERT(ADDR(.vector_table) >= ORIGIN(APP) && ADDR(.app_header) >= ORIGIN(APP)
       && ADDR(.text) >= ORIGIN(APP) && ADDR(.rodata) >= ORIGIN(APP)
       && ADDR(.gnu.sgstubs) >= ORIGIN(APP) && __sidata >= ORIGIN(APP), "
ERROR(ipmc): an output section lies below APP, in the bootloader or the FRU flash store");

ASSERT(__sidata + SIZEOF(.data) <= ORIGIN(APP) + LENGTH(APP), "
ERROR(ipmc): the flash image does not end inside the APP region (sectors 3-7, 464 KB)");

ASSERT(__veneer_limit <= ORIGIN(APP) + LENGTH(APP), "
ERROR(ipmc): the veneers do not end inside the APP region");

ASSERT(ADDR(.uninit) == ORIGIN(NOINIT) + 0x10, "
ERROR(ipmc): .uninit does not start at ORIGIN(NOINIT) + 0x10 (the boot request words come first)");

ASSERT(ADDR(.uninit) + SIZEOF(.uninit) <= ORIGIN(NOINIT) + LENGTH(NOINIT), "
ERROR(ipmc): .uninit does not end inside NOINIT (256 bytes)");

ASSERT(ORIGIN(RAM) % 4 == 0, "
ERROR(cortex-m-rt): the start of the RAM region must be 4-byte aligned");

ASSERT(__sdata % 4 == 0 && __edata % 4 == 0, "
BUG(cortex-m-rt): .data is not 4-byte aligned");

ASSERT(__sidata % 4 == 0, "
BUG(cortex-m-rt): the LMA of .data is not 4-byte aligned");

ASSERT(__sbss % 4 == 0 && __ebss % 4 == 0, "
BUG(cortex-m-rt): .bss is not 4-byte aligned");

ASSERT(__sheap % 4 == 0, "
BUG(cortex-m-rt): start of .heap is not 4-byte aligned");

ASSERT(_stack_start % 8 == 0, "
ERROR(cortex-m-rt): stack start address is not 8-byte aligned.
If you have set _stack_start, check it's set to an address which is a multiple of 8 bytes.
If you haven't, stack starts at the end of RAM by default. Check that both RAM
origin and length are set to multiples of 8 in the `memory.x` file.");

ASSERT(_stack_end % 4 == 0, "
ERROR(cortex-m-rt): end of stack is not 4-byte aligned");

ASSERT(_stack_start >= _stack_end, "
ERROR(cortex-m-rt): stack end address is not below stack start.");

ASSERT(_stack_start - _stack_end >= _stack_reserve, "
ERROR(ipmc): the RAM left for the stack is smaller than _stack_reserve (memory.x).
The statics (.data, .bss) fill too much of the RAM. Make IPMC_LOG_SIZE (capture log,
1024..=16384), IPMC_UART_RX_BUF, IPMC_UART_TX_BUF, IPMC_APP_UART_RX_BUF or IPMC_APP_UART_TX_BUF
in defs.rs smaller, or re-measure the stack and change _stack_reserve in memory.x.");

/* # Position checks */

/* ## .vector_table
 *
 * If the *start* of exception vectors is not 8 bytes past the start of the
 * vector table, then we somehow did not place the reset vector, which should
 * live 4 bytes past the start of the vector table.
 */
ASSERT(__exceptions == ADDR(.vector_table) + 0x8, "
BUG(cortex-m-rt): the reset vector is missing");

ASSERT(__eexceptions == ADDR(.vector_table) + 0x40, "
BUG(cortex-m-rt): the exception vectors are missing");

ASSERT(SIZEOF(.vector_table) > 0x40, "
ERROR(cortex-m-rt): The interrupt vectors are missing.
Possible solutions, from most likely to less likely:
- Link to a svd2rust generated device crate
- Check that you actually use the device/hal/bsp crate in your code
- Disable the 'device' feature of cortex-m-rt to build a generic application (a dependency
may be enabling it)
- Supply the interrupt handlers yourself. Check the documentation for details.");

ASSERT(ADDR(.vector_table) % MAX(128, 1 << LOG2CEIL(SIZEOF(.vector_table))) == 0, "
ERROR(cortex-m-rt): Interrupt vector table misalignment detected. The vector table must
be aligned to the larger of:
- 128 bytes
- The next power of two greater than or equal to its size");

/* ## .text */
ASSERT(ADDR(.vector_table) + SIZEOF(.vector_table) <= _stext, "
ERROR(cortex-m-rt): The .text section can't be placed inside the .vector_table section
Set _stext to an address greater than the end of .vector_table (See output of `nm`)");

ASSERT(_stext >= ORIGIN(APP) && _stext < ORIGIN(APP) + LENGTH(APP), "
ERROR(cortex-m-rt): The .text section must be placed inside the APP memory.
Set _stext to an address within the APP region.");

/* # Other checks */
ASSERT(SIZEOF(.got) == 0, "
ERROR(cortex-m-rt): .got section detected in the input object files
Dynamic relocations are not supported. If you are linking to C code compiled using
the 'cc' crate then modify your build script to compile the C code _without_
the -fPIC flag. See the documentation of the `cc::Build.pic` method for details.");
/* Do not exceed this mark in the error messages above                                    | */

/* Interrupt handler defaults of the device crate (stm32f4, feature rt). */
INCLUDE device.x
