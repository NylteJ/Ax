.global _start

.text
_start:
    st.d $zero, $sp, -8 # 测试内存写入（别忘了 *sp 是非法地址）
    st.d $a0, $sp, -16  # 测试内存读取
    li.d $a7, 233
    la $a0, msg
    li.d $a1, 14
    syscall 0
    li.d $a7, 93
    syscall 0

.data
msg: .ascii "Hello, World!\n"
