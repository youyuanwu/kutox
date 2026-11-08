.section .text
.global kutox_irq_register_probe
.type kutox_irq_register_probe, @function
kutox_irq_register_probe:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    sub rsp, 8
    mov [rsp], rsp
    mov rax, 0x101
    mov rcx, 0x102
    mov rdx, 0x103
    mov rbx, 0x104
    mov rbp, 0x105
    mov rsi, 0x106
    mov rdi, 0x107
    mov r8, 0x108
    mov r9, 0x109
    mov r10, 0x10a
    mov r11, 0x10b
    mov r12, 0x10c
    mov r13, 0x10d
    mov r14, 0x10e
    mov r15, 0x10f
    /* Seed DF and arithmetic flags; no Rust executes until CLD restores its ABI. */
    push 0xcd7
    popfq
    sti
    hlt
    pushfq
    cli
    cld
    cmp rax, 0x101
    jne .Lirq_register_bad
    cmp rcx, 0x102
    jne .Lirq_register_bad
    cmp rdx, 0x103
    jne .Lirq_register_bad
    cmp rbx, 0x104
    jne .Lirq_register_bad
    cmp rbp, 0x105
    jne .Lirq_register_bad
    cmp rsi, 0x106
    jne .Lirq_register_bad
    cmp rdi, 0x107
    jne .Lirq_register_bad
    cmp r8, 0x108
    jne .Lirq_register_bad
    cmp r9, 0x109
    jne .Lirq_register_bad
    cmp r10, 0x10a
    jne .Lirq_register_bad
    cmp r11, 0x10b
    jne .Lirq_register_bad
    cmp r12, 0x10c
    jne .Lirq_register_bad
    cmp r13, 0x10d
    jne .Lirq_register_bad
    cmp r14, 0x10e
    jne .Lirq_register_bad
    cmp r15, 0x10f
    jne .Lirq_register_bad
    lea rax, [rsp + 8]
    cmp rax, [rsp + 8]
    jne .Lirq_register_bad
    mov rax, [rsp]
    and eax, 0xed7
    cmp eax, 0xed7
    jne .Lirq_register_bad
    xor eax, eax
    jmp .Lirq_register_done
.Lirq_register_bad:
    mov eax, 1
.Lirq_register_done:
    add rsp, 16
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
.size kutox_irq_register_probe, .-kutox_irq_register_probe
