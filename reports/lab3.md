# Lab3：进程管理

## 一、功能总结

本实验迁移了 `sys_get_time`、`sys_mmap` 和 `sys_munmap`，使其适配第五章进程结构。实现了 `sys_spawn`，可加载指定程序并创建子进程。实现了 `sys_set_priority` 和 Stride 调度算法，根据进程优先级分配 CPU 时间片，并处理 stride 整数溢出问题。

## 二、问答题

### 1. Stride 溢出问题

假设使用 8 位无符号整数，两个进程的 pass 都为 10：P1 的 stride 为 255，P2 的 stride 为 250。P2 运行一个时间片后，`250 + 10 = 260`，在 8 位整数中溢出为 4。如果直接按普通数值比较，会认为 4 小于 255，从而再次调度 P2，但理论上下一次应该调度 P1。

### 2. 为什么要求 priority >= 2？

进程每次运行后增加 `pass = BIG_STRIDE / priority`。当 `priority >= 2` 时，`pass <= BIG_STRIDE / 2`。调度器始终选择 stride 最小的进程，因此进程间 stride 的有效差值不会超过数值范围的一半。即使发生整数回绕，也可以通过环形数值比较判断正确顺序。

### 3. Stride 比较器

```rust
fn stride_less(lhs: usize, rhs: usize) -> bool {
    lhs != rhs && lhs.wrapping_sub(rhs) > usize::MAX / 2
}
```

使用 `wrapping_sub` 计算环形差值。当差值超过整数范围的一半时，认为发生了回绕，实际顺序与普通无符号数值比较相反。

## 荣誉准则

我保证本报告如实记录本人对实验实现和原理的理解，未伪造实验结果；参考课程资料的部分均用于学习和分析。

