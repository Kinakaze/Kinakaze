/* Build: clang --target=x86_64-linux-gnu -nostdlib -static -fuse-ld=lld
 *        -Wl,-e,_start RawVforkStackProbe.S -o raw-vfork-stack-probe
 * Exercise musl's CLONE_VM | CLONE_VFORK | SIGCHLD shape with a child stack.
 */
.intel_syntax noprefix
.section .text
.global _start
_start:
    xor ebp, ebp
clone_again:
    mov r12, rsp
    lea rsi, [rip + child_stack_end]
    mov edi, 0x4111
    xor edx, edx
    xor r10d, r10d
    xor r8d, r8d
    mov eax, 56
    syscall
    test rax, rax
    js failed
    jz child
    cmp rsp, r12
    jne failed
    mov rdi, rax
    lea rsi, [rip + child_status]
    xor edx, edx
    xor r10d, r10d
    mov eax, 61
    syscall
    test rax, rax
    js failed
    mov eax, dword ptr [rip + child_status]
    test ebp, ebp
    jnz check_exec
    cmp eax, 42 << 8
    jne failed
    inc ebp
    jmp clone_again
check_exec:
    test eax, eax
    jne failed
    mov edi, 1
    lea rsi, [rip + success]
    mov edx, OFFSET success_length
    mov eax, 1
    syscall
    xor edi, edi
    mov eax, 60
    syscall
    ud2
child:
    cmp rsp, rsi
    jne failed
    test ebp, ebp
    jnz child_exec
    mov edi, 42
    mov eax, 60
    syscall
    ud2
child_exec:
    lea rdi, [rip + executable]
    lea rsi, [rip + arguments]
    lea rdx, [rip + environment]
    mov eax, 59
    syscall
failed:
    mov edi, 111
    mov eax, 60
    syscall
    ud2
.section .rodata
success: .ascii "RAW_VFORK_STACK_OK\n"
success_end:
.equ success_length, success_end - success
executable: .asciz "/bin/true"
.section .data
.balign 8
arguments: .quad executable, 0
environment: .quad 0
child_status: .long 0
.section .bss
.balign 16
child_stack: .skip 65536
child_stack_end:
.section .note.GNU-stack,"",@progbits
