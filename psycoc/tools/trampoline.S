    .text
    .code16
    .globl tramp_start
tramp_start:
    cli
    cld
    mov %cs, %ax
    mov %ax, %ds
    mov %ax, %ss
    mov $0x0FF0, %sp
    xor %ebx, %ebx
    mov %ax, %bx
    shl $4, %ebx
    lea (gdt - tramp_start)(%ebx), %eax
    mov %eax, (gdt_ptr - tramp_start + 2)
    lgdtl (gdt_ptr - tramp_start)
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0
    lea (pm32 - tramp_start)(%ebx), %eax
    pushl $0x08
    pushl %eax
    lretl

    .code32
pm32:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    lea 0x0FF0(%ebx), %esp
    mov %cr4, %eax
    or $0x20, %eax
    mov %eax, %cr4
    mov (cr3_slot - tramp_start)(%ebx), %eax
    mov %eax, %cr3
    mov $0xC0000080, %ecx
    rdmsr
    or $0x100, %eax
    wrmsr
    mov %cr0, %eax
    or $0x80010001, %eax
    mov %eax, %cr0
    lea (lm64 - tramp_start)(%ebx), %eax
    pushl $0x18
    pushl %eax
    lret

    .code64
lm64:
    movl %ebx, %ebx
    mov (stack_slot - tramp_start)(%rbx), %rsp
    mov (entry_slot - tramp_start)(%rbx), %rax
    jmp *%rax

    .align 8
gdt:
    .quad 0
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x00AF9A000000FFFF
gdt_end:
gdt_ptr:
    .word gdt_end - gdt - 1
    .long 0
    .align 8
    .globl cr3_slot
cr3_slot:
    .quad 0
    .globl stack_slot
stack_slot:
    .quad 0
    .globl entry_slot
entry_slot:
    .quad 0
    .globl tramp_end
tramp_end:
