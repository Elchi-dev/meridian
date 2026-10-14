/* The kernel runs from the top 2 GiB of the address space, where the
   kernel code model wants it. Code, read-only data and writable data each
   start on a page of their own, so the loader can map each with exactly
   the rights its segment asks for. The kernel is position independent;
   the loader applies its relocations if it ever puts it elsewhere. */

ENTRY(kernel_start)

PHDRS
{
    text    PT_LOAD    FLAGS(5);  /* read, execute */
    rodata  PT_LOAD    FLAGS(4);  /* read */
    data    PT_LOAD    FLAGS(6);  /* read, write */
    dynamic PT_DYNAMIC FLAGS(6);
}

SECTIONS
{
    . = 0xffffffff80000000;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata
    .dynsym   : { *(.dynsym) } :rodata
    .dynstr   : { *(.dynstr) } :rodata
    .gnu.hash : { *(.gnu.hash) } :rodata
    .hash     : { *(.hash) } :rodata
    .rela.dyn : { *(.rela.dyn .rela.*) } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)
    } :data
    .data.rel.ro : { *(.data.rel.ro .data.rel.ro.*) } :data
    .got : { *(.got .got.*) } :data
    .dynamic : { *(.dynamic) } :data :dynamic

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
        *(.interp)
    }
}
