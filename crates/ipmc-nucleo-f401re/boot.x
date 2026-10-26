/* boot.x - the linker script of the sector-0 bootloader ipmc-boot (build.rs passes -Tboot.x for
   that binary alone; the runtime has link.x).

   Derived from link.x.in of cortex-m-rt 0.7.7 (https://github.com/rust-embedded/cortex-m,
   licensed MIT OR Apache-2.0, Copyright the Cortex-M Developers; the developer notes and the
   checks of that file are kept), through this crate's link.x. Differences from link.x:

   - The whole image sits in BOOT (flash sector 0, 16 KB from 0x0800_0000): .vector_table first,
     then .text, .rodata and the .data load image. There is no .app_header and nothing in STORE
     or APP.
   - The RAM sections (.data, .bss, the stack at the top of RAM) sit in RAM, above NOINIT. No
     output section sits in NOINIT (the boot request words and the runtime's .uninit live there
     and survive the bootloader), and .uninit is empty.
   - The image must end inside BOOT: the link fails with the region overflow, and the assert
     below names the size budget of Documentation/ipmc_boot_plan.md 3.3 (design target 12 KB,
     hard limit 16 KB; gen.ps1 also runs `fwimage size` on ipmc-boot.bin).
   - _stack_reserve: the RAM that must stay free for the stack above the statics (the UART rings
     are the largest statics).

   Peer: flash_map.x (the regions and symbols), src/boot/ (the bootloader). */

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
/* This is flash_map.x, the one place of every address */
INCLUDE flash_map.x

/* Stack reservation: the stack of the bootloader (the SHA-256 state and the line editors are the
   largest frames) needs a few KB; the rest of the 95.75 KB of RAM is the stack. */
_stack_reserve = 8K;

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
  .vector_table ORIGIN(BOOT) :
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
  } > BOOT

  /* The alignment of .text is the maximum alignment required by its input sections, which
     can be more than 4 (e.g. `.p2align` directives inside inline assembly). Since the
     start address of .text is set explicitly, the linker won't align it automatically, so do it
     here. The program starts after the vector table. */
  PROVIDE(_stext = ALIGN(ADDR(.vector_table) + SIZEOF(.vector_table), ALIGNOF(.text)));

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
  } > BOOT

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
  } > BOOT

  /* ## Sections in RAM */
  /* ### .data */
  .data : ALIGN(4)
  {
    . = ALIGN(4);
    __sdata = .;
    *(.data .data.*);
    . = ALIGN(4); /* 4-byte align the end (VMA) of this section */
  } > RAM AT>BOOT
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
  } > BOOT
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

  /* ### .uninit: empty in the bootloader (asserted below); NOINIT is not used by any section */
  .uninit (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    __suninit = .;
    *(.uninit .uninit.*);
    . = ALIGN(4);
    __euninit = .;
  } > RAM

  /* Align `__sheap` and `_stack_end` pointers to 4 bytes */
  . = ALIGN(4);

  /* Place the heap start and stack end at the end of allocated RAM */
  PROVIDE(__sheap = .);
  PROVIDE(_stack_end = .);

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
ASSERT(ORIGIN(BOOT) % 4 == 0, "
ERROR(cortex-m-rt): the start of the BOOT region must be 4-byte aligned");

/* # Image placement checks (not in cortex-m-rt) */
ASSERT(ADDR(.vector_table) == ORIGIN(BOOT), "
ERROR(ipmc): the vector table is not at the start of BOOT (0x0800_0000)");

ASSERT(__sidata + SIZEOF(.data) <= ORIGIN(BOOT) + LENGTH(BOOT), "
ERROR(ipmc): the bootloader image does not fit sector 0 (BOOT, 16 KB). The design target is 12 KB
(75 %) and the hard limit 16 KB (Documentation/ipmc_boot_plan.md 3.3). Look at the sizes with
`arm-none-eabi-nm --size-sort -S` and remove code (no core::fmt, no panic path with a message).");

ASSERT(__veneer_limit <= ORIGIN(BOOT) + LENGTH(BOOT), "
ERROR(ipmc): the veneers do not end inside the BOOT region");

ASSERT(ADDR(.vector_table) >= ORIGIN(BOOT) && ADDR(.text) >= ORIGIN(BOOT)
       && ADDR(.rodata) >= ORIGIN(BOOT) && __sidata >= ORIGIN(BOOT), "
ERROR(ipmc): an output section lies outside BOOT");

ASSERT(ADDR(.data) >= ORIGIN(RAM) && ADDR(.bss) >= ORIGIN(RAM) && _stack_end >= ORIGIN(RAM), "
ERROR(ipmc): a RAM section lies in NOINIT, which holds the boot request and the runtime .uninit");

ASSERT(SIZEOF(.uninit) == 0, "
ERROR(ipmc): the bootloader has an .uninit section; NOINIT belongs to the boot request and the
runtime (a `link_section = .uninit` static must not reach this image)");

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
ERROR(ipmc): the RAM left for the stack is smaller than _stack_reserve (boot.x).
The statics (.data, .bss) fill too much of the RAM. Make IPMC_UART_RX_BUF, IPMC_UART_TX_BUF,
IPMC_APP_UART_RX_BUF or IPMC_APP_UART_TX_BUF in defs.rs smaller.");

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

ASSERT(_stext >= ORIGIN(BOOT) && _stext < ORIGIN(BOOT) + LENGTH(BOOT), "
ERROR(cortex-m-rt): The .text section must be placed inside the BOOT memory.
Set _stext to an address within the BOOT region.");

/* # Other checks */
ASSERT(SIZEOF(.got) == 0, "
ERROR(cortex-m-rt): .got section detected in the input object files
Dynamic relocations are not supported. If you are linking to C code compiled using
the 'cc' crate then modify your build script to compile the C code _without_
the -fPIC flag. See the documentation of the `cc::Build.pic` method for details.");
/* Do not exceed this mark in the error messages above                                    | */

/* Interrupt handler defaults of the device crate (stm32f4, feature rt). */
INCLUDE device.x
