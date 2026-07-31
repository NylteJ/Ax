#!/bin/sh
# LoongArch setup: replace RISC-V base-name binaries with symlinks to -la versions
/bin/busybox-la rm -f /bin/busybox /bin/agent_lite /bin/simple_llm_server
/bin/busybox-la ln -sf busybox-la /bin/busybox
/bin/busybox-la ln -sf agent_lite-la /bin/agent_lite
/bin/busybox-la ln -sf simple_llm_server-la /bin/simple_llm_server
