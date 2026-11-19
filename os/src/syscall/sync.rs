use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;

const DEADLOCK_ERROR: isize = -0xdead;

fn current_tid() -> usize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid
}
/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() - 1
    };
    process_inner.deadlock.add_mutex(id);
    id as isize
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let mutex = match process_inner
        .mutex_list
        .get(mutex_id)
        .and_then(|mutex| mutex.as_ref())
    {
        Some(mutex) => Arc::clone(mutex),
        None => return -1,
    };
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.mutex_available[mutex_id] > 0 {
        process_inner.deadlock.mutex_available[mutex_id] -= 1;
        process_inner.deadlock.mutex_allocation[tid][mutex_id] += 1;
        drop(process_inner);
        mutex.lock();
        return 0;
    }

    process_inner.deadlock.mutex_request[tid][mutex_id] += 1;
    if process_inner.deadlock.enabled && process_inner.deadlock.would_deadlock() {
        process_inner.deadlock.mutex_request[tid][mutex_id] -= 1;
        return DEADLOCK_ERROR;
    }
    drop(process_inner);
    mutex.lock();

    let mut process_inner = process.inner_exclusive_access();
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.mutex_request[tid][mutex_id] > 0 {
        process_inner.deadlock.mutex_request[tid][mutex_id] -= 1;
        if process_inner.deadlock.mutex_available[mutex_id] > 0 {
            process_inner.deadlock.mutex_available[mutex_id] -= 1;
        }
        process_inner.deadlock.mutex_allocation[tid][mutex_id] += 1;
    }
    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let mutex = match process_inner
        .mutex_list
        .get(mutex_id)
        .and_then(|mutex| mutex.as_ref())
    {
        Some(mutex) => Arc::clone(mutex),
        None => return -1,
    };
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.mutex_allocation[tid][mutex_id] == 0 {
        return -1;
    }
    process_inner.deadlock.mutex_allocation[tid][mutex_id] -= 1;
    drop(process_inner);
    let waking_tid = mutex.unlock();

    let mut process_inner = process.inner_exclusive_access();
    if let Some(waking_tid) = waking_tid {
        process_inner.deadlock.ensure_thread(waking_tid);
        if process_inner.deadlock.mutex_request[waking_tid][mutex_id] > 0 {
            process_inner.deadlock.mutex_request[waking_tid][mutex_id] -= 1;
            process_inner.deadlock.mutex_allocation[waking_tid][mutex_id] += 1;
        }
        process_inner.deadlock.mutex_available[mutex_id] = 0;
    } else {
        process_inner.deadlock.mutex_available[mutex_id] = 1;
    }
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    process_inner.deadlock.add_semaphore(id, res_count);
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let sem = match process_inner
        .semaphore_list
        .get(sem_id)
        .and_then(|sem| sem.as_ref())
    {
        Some(sem) => Arc::clone(sem),
        None => return -1,
    };
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.semaphore_allocation[tid][sem_id] > 0 {
        process_inner.deadlock.semaphore_allocation[tid][sem_id] -= 1;
    }
    drop(process_inner);
    let waking_tid = sem.up();

    let mut process_inner = process.inner_exclusive_access();
    if let Some(waking_tid) = waking_tid {
        process_inner.deadlock.ensure_thread(waking_tid);
        if process_inner.deadlock.semaphore_request[waking_tid][sem_id] > 0 {
            process_inner.deadlock.semaphore_request[waking_tid][sem_id] -= 1;
            process_inner.deadlock.semaphore_allocation[waking_tid][sem_id] += 1;
        }
    } else {
        process_inner.deadlock.semaphore_available[sem_id] += 1;
    }
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let sem = match process_inner
        .semaphore_list
        .get(sem_id)
        .and_then(|sem| sem.as_ref())
    {
        Some(sem) => Arc::clone(sem),
        None => return -1,
    };
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.semaphore_available[sem_id] > 0 {
        process_inner.deadlock.semaphore_available[sem_id] -= 1;
        process_inner.deadlock.semaphore_allocation[tid][sem_id] += 1;
        drop(process_inner);
        sem.down();
        return 0;
    }

    process_inner.deadlock.semaphore_request[tid][sem_id] += 1;
    if process_inner.deadlock.enabled && process_inner.deadlock.would_deadlock() {
        process_inner.deadlock.semaphore_request[tid][sem_id] -= 1;
        return DEADLOCK_ERROR;
    }
    drop(process_inner);
    sem.down();

    let mut process_inner = process.inner_exclusive_access();
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.semaphore_request[tid][sem_id] > 0 {
        process_inner.deadlock.semaphore_request[tid][sem_id] -= 1;
        if process_inner.deadlock.semaphore_available[sem_id] > 0 {
            process_inner.deadlock.semaphore_available[sem_id] -= 1;
        }
        process_inner.deadlock.semaphore_allocation[tid][sem_id] += 1;
    }
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_tid();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let condvar = match process_inner
        .condvar_list
        .get(condvar_id)
        .and_then(|condvar| condvar.as_ref())
    {
        Some(condvar) => Arc::clone(condvar),
        None => return -1,
    };
    let mutex = match process_inner
        .mutex_list
        .get(mutex_id)
        .and_then(|mutex| mutex.as_ref())
    {
        Some(mutex) => Arc::clone(mutex),
        None => return -1,
    };
    process_inner.deadlock.ensure_thread(tid);
    if process_inner.deadlock.mutex_allocation[tid][mutex_id] == 0 {
        return -1;
    }
    process_inner.deadlock.mutex_allocation[tid][mutex_id] -= 1;
    let has_waiter = process_inner
        .deadlock
        .mutex_request
        .iter()
        .any(|row| row[mutex_id] > 0);
    process_inner.deadlock.mutex_available[mutex_id] = usize::from(!has_waiter);
    drop(process_inner);
    condvar.wait(mutex);

    let mut process_inner = process.inner_exclusive_access();
    process_inner.deadlock.ensure_thread(tid);
    process_inner.deadlock.mutex_available[mutex_id] = 0;
    process_inner.deadlock.mutex_allocation[tid][mutex_id] += 1;
    0
}
/// enable deadlock detection syscall
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!("kernel: sys_enable_deadlock_detect enabled={}", enabled);
    current_process().inner_exclusive_access().deadlock.enabled = enabled != 0;
    0
}
