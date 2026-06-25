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