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
