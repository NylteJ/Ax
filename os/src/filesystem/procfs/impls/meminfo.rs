use crate::simple_file;

simple_file!(file: MemInfoFile, dentry: MemInfoDEntry; content: {
    // 摘自 NoAxiom 的 kernel/src/include/fs.rs
    // 我说功利是对的
    // 顺带一提，这不属于 GPL 2 的保护范畴，因为其没有任何独创性
    const STR: &[u8] =
br"MemTotal:	16251136 KB
MemFree:	327680 KB
MemAvailable:	15923456 KB
Buffers:	373336 KB
Cached:	10391984 KB
SwapCached:	0 KB
SwapTotal:	4194300 KB
SwapFree:	4194300 KB
Shmem:	0 KB
Slab:	0 KB
";

    STR
});
