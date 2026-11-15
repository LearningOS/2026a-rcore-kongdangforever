//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;


/// Stride 调度使用的公共大步长。
const BIG_STRIDE: usize = 1 << 20;

///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// 在可能发生整数回绕时，判断 lhs 是否表示更小的 stride。
fn stride_less(lhs: usize, rhs: usize) -> bool {
    lhs != rhs && lhs.wrapping_sub(rhs) > usize::MAX / 2
}

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty() {
            return None;
        }

        let mut min_index = 0;
        let mut min_stride = {
            let inner = self.ready_queue[0].inner_exclusive_access();
            inner.stride
        };

        for (index, task) in self.ready_queue.iter().enumerate().skip(1) {
            let stride = {
                let inner = task.inner_exclusive_access();
                inner.stride
            };

            if stride_less(stride, min_stride) {
                min_stride = stride;
                min_index = index;
            }
        }

        let task = self.ready_queue.remove(min_index).unwrap();

        {
            let mut inner = task.inner_exclusive_access();
            let pass = BIG_STRIDE / inner.priority;
            inner.stride = inner.stride.wrapping_add(pass);
        }

        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
