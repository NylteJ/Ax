using size_t = decltype(0uz);

static long print(const char* str, size_t len) {
    long ret;
    
#ifdef __riscv
    asm volatile (
        "li   a7, 233\n"
        "mv   a0, %1\n"
        "mv   a1, %2\n"
        "ecall\n"
        "mv   %0, a0\n"
        : "=r"(ret)
        : "r"(str), "r"(len)
        : "a0", "a1", "a2", "a7"
    );
#else
    asm volatile (
        "li.d   $a7, 233\n"
        "move   $a0, %1\n"
        "move   $a1, %2\n"
        "syscall 0\n"
        "move   %0, $a0\n"
        : "=r"(ret)
        : "r"(str), "r"(len)
        : "$a0", "$a1", "$a2", "$a7"
    );
#endif
    
    return ret;
}

template <size_t N>
static long print(const char (&str)[N]) {
    return print(str, N - 1);
}

template <class... Args>
static long print(const Args&... args)
    requires (sizeof...(Args) > 1)
{
    return (print(args) || ...);
}

[[noreturn]]
static void exit(long code) {
#ifdef __riscv
    asm volatile (
        "li   a7, 93\n"
        "mv   a0, %0\n"
        "ecall"
        :
        : "r"(code)
        : "a0", "a7"
    );
#else
    asm volatile (
        "li.d   $a7, 93\n"
        "move   $a0, %0\n"
        "syscall 0"
        :
        : "r"(code)
        : "$a0", "$a7"
    );
#endif
    __builtin_unreachable();
}

static long fork() {
    long ret;

#ifdef __riscv
    asm volatile (
        "li   a7, 220\n"
        "ecall\n"
        "mv   %0, a0\n"
        : "=r"(ret)
        :
        : "a0", "a7"
    );
#else
    asm volatile (
        "li.d   $a7, 220\n"
        "syscall 0\n"
        "move   %0, $a0\n"
        : "=r"(ret)
        :
        : "$a0", "$a7"
    );
#endif

    return ret;
}

static long execve(const char* path) {
    long ret;

#ifdef __riscv
    asm volatile (
        "li   a7, 221\n"
        "mv   a0, %1\n"
        "ecall\n"
        "mv   %0, a0\n"
        : "=r"(ret)
        : "r"(path)
        : "a0", "a7"
    );
#else
    asm volatile (
        "li.d   $a7, 221\n"
        "move   $a0, %1\n"
        "syscall 0\n"
        "move   %0, $a0\n"
        : "=r"(ret)
        : "r"(path)
        : "$a0", "$a7"
    );
#endif

    return ret;
}

#define assert(...) static_cast<void>(static_cast<bool>(__VA_ARGS__) || ((print("Assertion failed: " #__VA_ARGS__ "\nPos: " __FILE__ "（行号请见返回值）\n") && (exit(-100000), true)) || (exit(-__LINE__), true)))

long long zeros[16384 + 777];
constexpr char msg[] = "Hello from user!\n";

char zeros_impossible_to_eagerly_load[114514191981uz];

extern "C" [[noreturn]] void _start() {
    volatile bool should_exit = false;
    if (should_exit) {
        exit(reinterpret_cast<long>(&zeros[0]));
    }

    const auto ret = print(msg, sizeof(msg) - 1);
    assert(ret == 0);

    for (const auto& zero : zeros) {
        assert(zero == 0);
    }

    volatile auto& zeros2 = zeros_impossible_to_eagerly_load;

    assert(static_cast<volatile char&>(zeros2[7777777]) == 0);

    zeros2[7777777] = 27;
    zeros2[7777778] = 78;

    assert(zeros2[7777776] == 0);
    assert(zeros2[7777777] == 27);
    assert(zeros2[7777778] == 78);
    assert(zeros2[7777779] == 0);

    for (int i = 0; i < 260; ++i) {
        const char c = i % 26 + ((i / 26) % 2 ? 'a' : 'A'); // 我们甚至意外地发现了一个（极大概率的）GCC bug！
        print(&c, 1uz);
    }

    print("很好，下面试试 fork\n");

    fork();
    fork();
    fork();

    for (int i = 0; i < 260; ++i) {
        const char c = i % 26 + ((i / 26) % 2 ? 'a' : 'A');
        print(&c, 1uz);
    }

    constexpr auto path =
#ifdef __riscv
        "/tmp/../../rv_hlwd"
#else
        "/tmp/../../la_hlwd"
#endif
    ;

    const auto execve_ret = execve(path);
    assert(execve_ret != 0);
    exit(execve_ret);
    
    // exit(last_fork_ret);
}

// 它存在的意义只是给代码段加点东西（也因此不能是 static）
long func(int a1, int a2, short a3, char a4) {
    for (int i = 0; i < 100 + a4; i++) {
        for (int j = 0; j < 100 - a1; j++) {
            for (int k = 0; k < 100 + a2; k++) {
                a4 += a3;
            }

            a2 *= a1;
        }

        a1 -= a4;
    }

    return a1 + a2 * a3 - a4;
}

// 类似
template <class T, size_t N>
struct array {
    T data[N];
};

extern constexpr auto huge = [] {
    array<size_t, 0721> ret;

    size_t i = 0;
    for (auto&& x : ret.data) {
        x = i;
        i++;
    }

    return ret;
}();
