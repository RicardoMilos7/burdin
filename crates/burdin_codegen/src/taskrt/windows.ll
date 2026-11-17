; The task runtime for Windows (spec/tasks.md, TK2). It has the same functions as `unix.ll`: threads are
; `CreateThread`, locks are slim reader/writer locks and condition variables, and an execution context is a fiber, so
; the system commits the stack as it is used and supplies the guard page and the stack-overflow exception.

declare ptr @malloc(i64)
declare ptr @calloc(i64, i64)
declare void @free(ptr)
declare void @abort()
declare ptr @CreateThread(ptr, i64, ptr, ptr, i32, ptr)
declare i32 @CloseHandle(ptr)
declare void @InitializeSRWLock(ptr)
declare void @AcquireSRWLockExclusive(ptr)
declare void @ReleaseSRWLockExclusive(ptr)
declare void @InitializeConditionVariable(ptr)
declare i32 @SleepConditionVariableSRW(ptr, ptr, i32, i32)
declare void @WakeConditionVariable(ptr)
declare void @WakeAllConditionVariable(ptr)
declare i32 @SwitchToThread()
declare ptr @ConvertThreadToFiberEx(ptr, i32)
declare ptr @CreateFiberEx(i64, i64, i32, ptr, ptr)
declare void @SwitchToFiber(ptr)
declare void @DeleteFiber(ptr)
declare i32 @GetActiveProcessorCount(i16)

@burdin_rt_tls_worker = internal thread_local global ptr null
@burdin_rt_spin = internal global i32 0
@burdin_rt_global = internal global ptr null
@burdin_rt_config = internal global i64 0

; ---- threads ----------------------------------------------------------------------------------------------------

define i32 @burdin_rt_thread_main(ptr %arg) {
  %code = load ptr, ptr %arg
  %envp = getelementptr i8, ptr %arg, i64 8
  %env = load ptr, ptr %envp
  call void @free(ptr %arg)
  call void %code(ptr %env, i1 true)
  ret i32 0
}

define void @burdin_rt_thread_start(ptr %code, ptr %env) {
  %e = call ptr @malloc(i64 16)
  store ptr %code, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  store ptr %env, ptr %envp
  %h = call ptr @CreateThread(ptr null, i64 0, ptr @burdin_rt_thread_main, ptr %e, i32 0, ptr null)
  %bad = icmp eq ptr %h, null
  br i1 %bad, label %fail, label %ok
fail:
  call void @abort()
  unreachable
ok:
  %r = call i32 @CloseHandle(ptr %h)
  ret void
}

define void @burdin_rt_thread_yield() {
  %r = call i32 @SwitchToThread()
  ret void
}

define i64 @burdin_rt_cpu_count() {
  %n = call i32 @GetActiveProcessorCount(i16 65535)
  %wide = zext i32 %n to i64
  %small = icmp slt i64 %wide, 1
  %v = select i1 %small, i64 1, i64 %wide
  ret i64 %v
}

; ---- locks ------------------------------------------------------------------------------------------------------

define ptr @burdin_rt_mutex_new() {
  %m = call ptr @calloc(i64 1, i64 16)
  call void @InitializeSRWLock(ptr %m)
  ret ptr %m
}
define void @burdin_rt_mutex_lock(ptr %m) {
  call void @AcquireSRWLockExclusive(ptr %m)
  ret void
}
define void @burdin_rt_mutex_unlock(ptr %m) {
  call void @ReleaseSRWLockExclusive(ptr %m)
  ret void
}
define void @burdin_rt_mutex_free(ptr %m) {
  call void @free(ptr %m)
  ret void
}
define ptr @burdin_rt_cond_new() {
  %c = call ptr @calloc(i64 1, i64 16)
  call void @InitializeConditionVariable(ptr %c)
  ret ptr %c
}
define void @burdin_rt_cond_wait(ptr %c, ptr %m) {
  %r = call i32 @SleepConditionVariableSRW(ptr %c, ptr %m, i32 -1, i32 0)
  ret void
}
define void @burdin_rt_cond_signal(ptr %c) {
  call void @WakeConditionVariable(ptr %c)
  ret void
}
define void @burdin_rt_cond_broadcast(ptr %c) {
  call void @WakeAllConditionVariable(ptr %c)
  ret void
}
define void @burdin_rt_cond_free(ptr %c) {
  call void @free(ptr %c)
  ret void
}

; ---- the two slots, and the lock for one-time initialization -------------------------------------------------------

define ptr @burdin_rt_tls_get() noinline {
  %v = load volatile ptr, ptr @burdin_rt_tls_worker
  ret ptr %v
}
define void @burdin_rt_tls_set(ptr %p) noinline {
  store volatile ptr %p, ptr @burdin_rt_tls_worker
  ret void
}
define ptr @burdin_rt_global_get() {
  %v = load atomic ptr, ptr @burdin_rt_global acquire, align 8
  ret ptr %v
}
define void @burdin_rt_global_set(ptr %p) {
  store atomic ptr %p, ptr @burdin_rt_global release, align 8
  ret void
}
define i64 @burdin_rt_config_get() {
  %v = load atomic i64, ptr @burdin_rt_config acquire, align 8
  ret i64 %v
}
define void @burdin_rt_config_set(i64 %n) {
  store atomic i64 %n, ptr @burdin_rt_config release, align 8
  ret void
}
define void @burdin_rt_spin_lock() {
  br label %try
try:
  %r = cmpxchg ptr @burdin_rt_spin, i32 0, i32 1 acquire monotonic
  %ok = extractvalue { i32, i1 } %r, 1
  br i1 %ok, label %done, label %spin
spin:
  %y = call i32 @SwitchToThread()
  br label %try
done:
  ret void
}
define void @burdin_rt_spin_unlock() {
  store atomic i32 0, ptr @burdin_rt_spin release, align 4
  ret void
}

; ---- execution contexts -------------------------------------------------------------------------------------------
; A context is { fiber }. `ctx_main` turns the calling thread into a fiber, so it can be switched away from and back
; to; `ctx_new` creates a fiber that reserves `size` bytes of stack and commits them as they are used.

define void @burdin_rt_ctx_entry(ptr %e) {
  %code = load ptr, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  %env = load ptr, ptr %envp
  call void @free(ptr %e)
  call void %code(ptr %env, i1 true)
  ; The body is done. By the contract with the scheduler (spec/tasks.md, TK2) the first word of the worker record the
  ; thread's slot points at is the worker's own context; control goes back there and never returns, because returning
  ; from a fiber would end the thread.
  %w = call ptr @burdin_rt_tls_get()
  %main = load ptr, ptr %w
  %fiber = load ptr, ptr %main
  call void @SwitchToFiber(ptr %fiber)
  call void @abort()
  unreachable
}

define ptr @burdin_rt_ctx_main() {
  %c = call ptr @calloc(i64 1, i64 16)
  %f = call ptr @ConvertThreadToFiberEx(ptr null, i32 0)
  store ptr %f, ptr %c
  ret ptr %c
}

define ptr @burdin_rt_ctx_new(i64 %size, ptr %code, ptr %env) {
  %e = call ptr @malloc(i64 16)
  store ptr %code, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  store ptr %env, ptr %envp
  %f = call ptr @CreateFiberEx(i64 0, i64 %size, i32 0, ptr @burdin_rt_ctx_entry, ptr %e)
  %bad = icmp eq ptr %f, null
  br i1 %bad, label %fail, label %ok
fail:
  call void @abort()
  unreachable
ok:
  %c = call ptr @calloc(i64 1, i64 16)
  store ptr %f, ptr %c
  ret ptr %c
}

define void @burdin_rt_ctx_switch(ptr %from, ptr %to) noinline {
  %f = load ptr, ptr %to
  call void @SwitchToFiber(ptr %f)
  ret void
}

define void @burdin_rt_ctx_free(ptr %ctx) {
  %f = load ptr, ptr %ctx
  call void @DeleteFiber(ptr %f)
  call void @free(ptr %ctx)
  ret void
}
