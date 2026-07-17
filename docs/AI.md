# 生成式 AI 的使用情况
1. `make check`，使用 DeepSeek 网页端，提示词：
```
编写一个 Makefile，其有一个目标 `check`，`check` 会递归地检测当前文件夹下有没有隐藏文件或隐藏文件夹（不包括 . 和 ..），有则报告对应的文件（夹）路径。应当有一个能够配置的白名单列表，并且白名单会自动包含 .gitignore 中的所有文件（夹）。如果不存在隐藏文件（夹），则报告并正常退出，否则异常退出。所有不在白名单中的隐藏文件和文件夹都应当被报告。可以的话，使用彩色输出。
```
有人工修改，其原始输出：
````
下面是一个符合要求的 `Makefile`，它提供了 `check` 目标，用于递归检查当前目录下的所有隐藏文件和文件夹（排除 `.` 和 `..`），并将不在白名单中的项以彩色输出报告。白名单可手动配置，并自动包含 `.gitignore` 中的所有条目（忽略空行和注释）。

```makefile
# 可手动配置的白名单（空格分隔）
WHITELIST ?=

# 颜色定义
RED    := \033[0;31m
GREEN  := \033[0;32m
NC     := \033[0m

.PHONY: check
check:
	@# 解析 .gitignore 中的条目，去除首尾空白、注释、空行和尾部斜杠
	@{ \
		GITIGNORE_LIST=$$( [ -f .gitignore ] && grep -v '^#' .gitignore | grep -v '^$$' | sed 's|^[[:space:]]*||;s|[[:space:]]*$$||;s|/$$||' || true ); \
		USER_LIST=$$(printf '%s\n' $(WHITELIST) | sed 's|^[[:space:]]*||;s|[[:space:]]*$$||;s|/$$||'); \
		FULL_WHITELIST=$$(printf '%s\n%s' "$$USER_LIST" "$$GITIGNORE_LIST" | sort -u | sed '/^$$/d'); \
		echo "$$FULL_WHITELIST" > .whitelist.tmp; \
		violations=0; \
		while IFS= read -r -d '' item; do \
			rel_path=$${item#./}; \
			if ! grep -Fxq "$$rel_path" .whitelist.tmp && ! grep -Fxq "$$(basename "$$rel_path")" .whitelist.tmp; then \
				echo "$(RED)Found hidden item not in whitelist: $$item$(NC)"; \
				violations=$$((violations + 1)); \
			fi; \
		done < <(find . -name ".*" ! -name "." ! -name ".." -print0); \
		rm -f .whitelist.tmp; \
		if [ $$violations -eq 0 ]; then \
			echo "$(GREEN)No hidden items outside whitelist found.$(NC)"; \
			exit 0; \
		else \
			echo "$(RED)Total violations: $$violations$(NC)"; \
			exit 1; \
		fi; \
	}
```

### 功能说明

1. **递归检测**  
   使用 `find . -name ".*" ! -name "." ! -name ".."` 查找当前目录及所有子目录下以点开头的文件和文件夹（自动排除 `.` 和 `..`）。

2. **白名单机制**  
   - 通过 `WHITELIST` 变量可手动添加白名单项（空格分隔）。
   - 自动读取项目根目录的 `.gitignore` 文件，将其中的非空、非注释行作为白名单条目。
   - 合并后的白名单会去除首尾空白、注释、空行以及末尾斜杠，避免匹配差异。

3. **匹配规则**  
   对于每个找到的隐藏项，检查其相对路径（去掉 `./` 前缀）或其基础名是否**精确匹配**白名单中的任意一行。若均不匹配，则视为违规并报告。

4. **彩色输出**  
   - 违规项以红色打印。
   - 通过检查时以绿色打印成功信息。

5. **退出状态**  
   - 无违规项：退出码 `0`。
   - 有违规项：退出码 `1`（异常退出）。

### 使用示例

```bash
# 直接运行检查
make check

# 自定义添加白名单项（例如 .env 和 .vscode/）
make check WHITELIST=".env .vscode"
```

### 注意事项

- `.gitignore` 中的通配符模式（如 `*.log`）不会被解析为模式匹配，仅作为精确名称或路径处理。若需支持通配符，建议在脚本中改用 `grep -E` 等工具扩展，但会增加复杂度。
- 白名单条目若为目录（如 `.git/`），其尾部斜杠会被自动去除，匹配时仍按名称处理。
- 脚本会生成临时文件 `.whitelist.tmp`，执行结束后自动删除。
````

2. 一处 bugfix，使用 Claude Code + DeepSeek V4 Pro（借由 CC Switch 实现），提示词：
```
目前此内核存在 bug，在写回 ext4 上的文件内容时，会导致磁盘损坏。请尝试调查并修复。你可以通过 `make regen-disk run-rv MODE=debug`
  重新生成磁盘镜像并编译运行（带日志）以复现和调查问题，并通过 `e2fsck -f -n ./disk.img` 检验磁盘是否正常。该内核存在很多其它不完善之处，因此 e2fsck
  输出一些错误是可以接受的，只要其超级块不会再损坏就够了（具体而言，这要求你消除 `e2fsck: Superblock invalid, trying backup blocks...e2fsck: Bad magic number in super-block while
  trying to open ./disk.img` 报错）。另外，尽管目前运行的测试脚本为 madvise 相关脚本，问题很有可能与之无关，而是完全位于文件系统。此外，edited_lib/exy4_rs
  也并不完全可靠，必要时你可以对其进行修改。
```
输出过长，不在此展示。

3. 找寻 ltp 中的高价值测试点，同样使用 Claude Code + DeepSeek V4 Pro，提示词：
```
当前目录下有一个 ltp_output.txt 文件，你需要编写一个脚本用于分析它。这个文件来自于 ltp 运行的输出，由多个测试点输出组成，测试点输出彼此不重叠。每个测试点由 RUN LTP CASE ... 开始，由 FAIL LTP CASE ... 结束（注意无论测试是否通过都以 FAIL ... 结束）。每个测试点会输出一些中间日志。我需要你 1. 分析出每个测试点的 TPASS 和 TFAIL、TBROK、TCONF 数目 2. 对每个测试点按某种规律排序，以找出“最有价值”的那些测试点。测试点的价值是由它全部通过时的 TPASS 个数决定的。因此，显而易见，TPASS 数目多的测试点更有价值。然而，有许多测试点，尽管它们的 TPASS 很少，但它们在正确通过后能提供的 TPASS 会很多（只是这份输出中它们失败了，从而没有提供 TPASS）。当然，你不可能依据失败结果确定成功时的结果，但你可以进行一些猜测。有的 TFAIL/TBROK/TCONF 会让整个测试立刻终止，此时一个测试可能只会产生一个条目（尽管如果它能通过的话也许可以产生很多 TPASS）；有的 TFAIL/TBROK/TCONF 和 TPASS 是一比一的关系，因此有多少 TFAIL 等就意味着它通过后会有多少 TPASS。无论如何，找出最有“潜力”的测试点的任务很可能需要你手动完成。不过，前面的几步机械性的操作非常建议让脚本代劳，因为这个文件大约有 700KB，不可能读完（你仍然可以读前几行或者后几行以观察模式）。你最后可以给我一份分析报告，包括哪些测试点是能确定的有价值，哪些测试点非常有潜力，等等。具体要给我多少个测试点，可以由你自行决定，建议是囊括所有显著有价值的。
```
输出过长，不在此展示。

4. 修改 smoltcp 库（最终 Agent 还修改了 managed 库）以支持自动扩展的 TCP Socket 缓冲区。同样使用 Claude Code + DeepSeek V4 Pro，提示词：
```
当前文件夹下是 Rust 的 smoltcp 库。它的 TCP Socket 必须使用固定大小的缓冲区，且不支持中途更换。请你调查并做出更改，让它的 TCP Socket 在用 Vec 作为缓冲区时，能够支持 Vec 的自动扩容，并支持设定扩容上限，只在超过上限时返回错误，其余时候直接扩容即可。如果 Vec 较难支持，你也可以改用 Box<[...]> 并手动扩容。你可以假设 alloc 可用（但不能假设 std 可用），且无需添加与 alloc 相关的 cfg。
```
输出过长，不在此展示。此后，由于 Agent 直接修改了 vendor 下的 managed 库，又进行了一轮对话：
```
请不要更改 vendor。如果你需要修改 managed 库，请将它移动到 edited_lib 中，然后修改 edited_lib 中的版本，并将 Cargo.toml 修改为指向这个版本。由于你已经修改了 vendor 中的版本，只要将修改后的版本移动到 edited_lib 中，并修改 Cargo.toml 中即可。
```
输出过长，不在此展示。

5. 修改 smoltcp 库以支持带 backlog 语义的 listen 操作。使用 Claude Code + DeepSeek V4 Pro，提示词：
```
当前文件夹下是稍加修改过的 Rust smoltcp 库。它的 TCP Socket 的 listen 不支持 backlog 机制。请你调查并修改库，让它支持 backlog。backlog 参数可以在 listen 时传入，语义和 Linux 的保持一致（比如，如果 listen 时 socket 已经出在 listen 状态且目标地址相同，则更新 backlog）。你可以假设 alloc 可用，且无需添加 alloc 的 cfg，但不能假设 std 可用。

如果你需要修改此库的第三方库，且这部分库原先不在 edited_lib（父文件夹）中，则它们位于 ../../vendor 中，你不应该直接修改 vendor 中的库，而应当将对应库复制到 edited_lib 下，然后修改这个副本，并修改 Cargo.toml 以改用这个修改后的库；如果不需要修改第三方库，忽略该指示即可。
```
输出过长，不在此展示。

6. 修改 smoltcp 库以支持 UDP Socket send_with 的错误传播功能，并调查其是否存在可能截断 packet 的 bug。使用 Claude Code + DeepSeek V4 Pro，提示词：
```
当前文件夹下是稍加修改过的 Rust smoltcp 库。其 UDP socket 的 send_with 函数不支持传播给定函数的错误（即，它要求 f 必须成功）。请你调查并作出修改，以让 f 能够返回 `Result<usize, SomeErr>`，并在 f 返回 Err 直接传递这个错误（可以新建一个 SendWithError 枚举并添加一个泛型变体）。

除此之外，send_with 函数目前或许可能会截断待发送的包。我不确定是不是这样，因为它的注释声称当会发生截断时其会返回 Err，但我似乎没有找到对应变体。请你顺便验证其是否能正确处理，如果不能，请修复它。如果 send_with 有问题，send 等函数可能也有同样的问题，但你不需要修复它们，专注于 send_with 即可。
```
最终报告：
```
所有更改均已完成，所有 607 个测试均已通过。以下是所做更改的摘要：

---
已修改的文件

1. src/storage/packet_buffer.rs

新增方法 enqueue_with（第 166-236 行）— 现有的 enqueue_with_infallible 的容错版本：

- 签名：pub fn enqueue_with<'b, F, E>(&'b mut self, max_size: usize, header: H, f: F) -> Result<Result<usize, E>, Full> where F: FnOnce(&'b mut [u8]) -> Result<usize, E>
- 采用与 enqueue_with_infallible 相同的容量/填充检查
- 将闭包包装后传递给 enqueue_many_with：成功时返回 (size, Ok(size))，失败时返回 (0, Err(e))
- 失败时不提交任何字节且不追加元数据条目
- 孤儿填充：若在闭包调用前已添加填充，失败后填充仍保留在缓冲区中。这是无害的——后续调用 dequeue_padding() 时会将其清理

更新了注释（第 74 行）——移除了过时的 "目前没有 enqueue_with()" 的注释。

2. src/socket/udp.rs

新增 SendWithError<E> 枚举（第 95-148 行）：

pub enum SendWithError<E> {
    Unaddressable,
    BufferFull,
    Custom(E),
}

带有条件 trait 实现：Debug、Clone（当 E: Clone）、Copy（当 E: Copy）、PartialEq/Eq（当 E: PartialEq/Eq）、Display（当 E: Display）、std::error::Error（当 E: std::error::Error + 'static，且通过 source() 正确链接）。

更改了 send_with 签名——现接受 F: FnOnce(&mut [u8]) -> Result<usize, E> 并返回 Result<usize, SendWithError<E>>。调用新的 enqueue_with 并映射错误：Full → BufferFull，Err(e) → Custom(e)。

修复了文档注释：
- send() 文档不再引用不存在的 Error::Truncated 变体
- send_with() 文档现在描述了容错闭包语义和 SendWithError 返回类型

3. 未修改的文件

根据你的要求，src/socket/icmp.rs 和 src/socket/raw.rs 中的 send_with 方法保持不变（未修改）。

---
截断分析

未发现实际的截断错误。 send() 文档中提到的 Err(Error::Truncated) 是一个过时的文档错误（Truncated 仅存在于 RecvError 中，不存在于 SendError 中）。当缓冲区空间不足时，实际行为是返回 SendError::BufferFull——数据包会被拒绝，绝不会被截断。send_with 的情况相同。这两个文档错误均已修复。
```

7. 修改 smoltcp 库以支持非轮询的发送和接收功能。使用 Claude Code + DeepSeek V4 Pro，提示词：
```
当前文件夹下是稍加修改过的 Rust smoltcp 库。其 Interface 不支持直接对特定 Socket 进行操作（只支持对 Socket 集进行操作，即 poll 系函数）。请你提取出一套特殊接口，以支持绕过 SocketSet 的高效 ingress 和 egress 操作。具体而言：
  - 对于 egress，外部会使用一个特定的 SocketMRef 而不是一整个 &mut SocketSet 调用它，Interface 只要对这一个 Socket 进行处理（发送其输出缓冲区内的数据）即可；
  - 对于 ingress，由于整个 SocketSet 系统被外部代理掉了，外部会先通过 Interface 读取一个包，得到它的地址，然后自行查找对应的 Socket，再对该 Socket 执行 ingress（将这个包对应的数据传输到 Socket 的输入缓冲区中）。这个 API 可以有多种设计方式，例如可以拆成两个函数，一个负责读包（并可以从包中解析出地址），另一个接收包和 SocketMRef，将数据实际传输进缓冲区；也可以只使用一个函数，但其接收一个函数，用于根据地址返回合适的 SocketMRef，但这会遇到生命周期问题，可能需要特别的解决方案（目前外部仓库使用锁管理 Socket，因此这个函数必须返回锁守卫，但锁守卫是外部库的自定义类型，在这里使用会需要较多的泛型，特别是会需要一个泛型的 SocketMRefGuard 之类的类型，非常麻烦）。你也可以选择任何你认为好的设计方式。

注：SocketMRef 是目前不存在的包装类型，你需要自己添加它。它和 smoltcp::socket::Socket 的关系类似于 Option<&mut T> 和 &mut Option<T> 的关系，即它不要求原始 Socket 包裹在 socket::Socket 中。

此外，不要过于顾虑 unsafe 的使用，如果一些 API 设计合理、清晰且高效，但会带来难以验证的跨 API 约定时，将 API 设为 unsafe 并把约定的保证义务交给调用方即可，不要为了安全性牺牲我前面提到的特性。此外，内部函数也可以合理地使用 unsafe 以提高效率。
```
