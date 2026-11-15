# Lab3：进程管理

## 一、功能总结

本实验迁移了 `sys_get_time`、`sys_mmap` 和 `sys_munmap`，使其适配第五章进程结构。实现了 `sys_spawn`，可加载指定程序并创建子进程。实现了 `sys_set_priority` 和 Stride 调度算法，根据进程优先级分配 CPU 时间片，并处理 stride 整数溢出问题。

## 二、问答题

### 1. Stride 溢出问题

假设使用 8 位无符号整数，两个进程的 pass 都为 10：

- P1 的 stride 为 255
- P2 的 stride 为 250

P2 运行一个时间片后：

```text
250 + 10 = 260
```

在 8 位无符号整数中会溢出为 4。如果直接按普通数值比较，会认为 4 小于 255，从而再次调度 P2，但理论上下一次应该调度 P1。

### 2. 为什么要求 priority >= 2？

进程每次运行后增加：

```text
pass = BIG_STRIDE / priority
```

当 `priority >= 2` 时：

```text
pass <= BIG_STRIDE / 2
```

调度器始终选择 stride 最小的进程，因此各进程的 stride 差值不会超过数值范围的一半。这样即使发生整数溢出，也可以通过环形数值比较判断正确顺序。

### 3. Stride 比较器

```rust
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let diff = self.0.wrapping_sub(other.0);

        if diff > u64::MAX / 2 {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        }
    }
}

impl PartialEq for Stride {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
```

使用 `wrapping_sub` 计算环形差值。当差值超过整数范围的一半时，认为发生了溢出，实际顺序与普通数值比较相反。