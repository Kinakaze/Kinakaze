/* Table-only branches with a renamed checked index, as emitted by clang for
 * Firefox's wasm2c XML tokenizer. Every GS load must reach the guest base. */
extern long syscall(long, ...);
static unsigned long values[16] = {
    0,0,0,0,0,0, 101,202,303,404,505,606,707,808,909,0
};
__asm__(
    ".pushsection .text.copied_switch,\"ax\",@progbits\n"
    ".local copied_switch\n.type copied_switch,@function\n"
    "copied_switch:\n.cfi_startproc\n"
    "lea .Lcopied_table(%rip),%r11\nmov %edi,%r9d\njmp 8f\n"
    "8: cmp $8,%r9d\nja 9f\nmov %r9d,%r8d\n"
    "movslq (%r11,%r8,4),%r8\nadd %r11,%r8\njmp *%r8\n"
    "0: mov %gs:48,%rax\nret\n1: mov %gs:56,%rax\nret\n"
    "2: mov %gs:64,%rax\nret\n3: mov %gs:72,%rax\nret\n"
    "4: mov %gs:80,%rax\nret\n5: mov %gs:88,%rax\nret\n"
    "6: mov %gs:96,%rax\nret\n7: mov %gs:104,%rax\nret\n"
    "10: mov %gs:112,%rax\nret\n9: xor %eax,%eax\nret\n"
    ".cfi_endproc\n.size copied_switch,.-copied_switch\n.popsection\n"
    ".pushsection .rodata\n.Lcopied_table:\n"
    ".long 0b-.Lcopied_table,1b-.Lcopied_table,2b-.Lcopied_table\n"
    ".long 3b-.Lcopied_table,4b-.Lcopied_table,5b-.Lcopied_table\n"
    ".long 6b-.Lcopied_table,7b-.Lcopied_table,10b-.Lcopied_table\n.popsection\n"
);
int probe(void) {
    unsigned long old;
    if (syscall(158, 0x1004, &old) || syscall(158, 0x1001, values)) return 1;
    unsigned long (*call)(unsigned);
    __asm__ volatile("lea copied_switch(%%rip),%0" : "=r"(call));
    int result = 0;
    for (unsigned index = 0; index < 9; ++index)
        if (call(index) != values[index + 6]) { result = index + 2; break; }
    if (call(9) != 0 || call(~0u) != 0) result = 11;
    if (syscall(158, 0x1001, old)) result = 12;
    return result;
}
