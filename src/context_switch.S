.section .text
.global kutox_context_switch
.type kutox_context_switch, @function
kutox_context_switch:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, [rsi]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
.size kutox_context_switch, .-kutox_context_switch

.global kutox_thread_start
.type kutox_thread_start, @function
kutox_thread_start:
    cld
    mov rdi, rsp
    sub rsp, 8
    call kutox_thread_entry
    ud2
.size kutox_thread_start, .-kutox_thread_start

.global kutox_thread_register_probe
.type kutox_thread_register_probe, @function
kutox_thread_register_probe:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    sub rsp, 8
    mov [rsp], rsp
    mov rbp, 0x11223344
    mov rbx, 0x22334455
    mov r12, 0x33445566
    mov r13, 0x44556677
    mov r14, 0x55667700
    mov r15, 0x66770011
    call kutox_thread_yield_probe
    test rax, rax
    jne .Lprobe_failed
    cmp [rsp], rsp
    jne .Lprobe_failed
    cmp rbp, 0x11223344
    jne .Lprobe_failed
    cmp rbx, 0x22334455
    jne .Lprobe_failed
    cmp r12, 0x33445566
    jne .Lprobe_failed
    cmp r13, 0x44556677
    jne .Lprobe_failed
    cmp r14, 0x55667700
    jne .Lprobe_failed
    cmp r15, 0x66770011
    jne .Lprobe_failed
    xor eax, eax
    jmp .Lprobe_done
.Lprobe_failed:
    mov eax, 1
.Lprobe_done:
    add rsp, 8
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
.size kutox_thread_register_probe, .-kutox_thread_register_probe

.section .note.GNU-stack,"",@progbits
