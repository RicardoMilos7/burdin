; The task runtime for Unix targets (spec/tasks.md, TK2): threads, locks, execution contexts, and the slots the
; scheduler in `std/src/task.bd` keeps. The code generator parses this text and links it into the program the first
; time a task primitive is used, so a program that never starts a task carries none of it. The `@NAME@` markers are
; filled in per target by `taskrt.rs`.

declare ptr @malloc(i64)
declare ptr @calloc(i64, i64)
declare void @free(ptr)
declare void @abort()
declare i64 @write(i32, ptr, i64)
declare i32 @pthread_create(ptr, ptr, ptr, ptr)
declare i32 @pthread_detach(i64)
declare i32 @pthread_mutex_init(ptr, ptr)
declare i32 @pthread_mutex_lock(ptr)
declare i32 @pthread_mutex_unlock(ptr)
declare i32 @pthread_mutex_destroy(ptr)
declare i32 @pthread_cond_init(ptr, ptr)
declare i32 @pthread_cond_wait(ptr, ptr)
declare i32 @pthread_cond_signal(ptr)
declare i32 @pthread_cond_broadcast(ptr)
declare i32 @pthread_cond_destroy(ptr)
declare i32 @sched_yield()
declare ptr @mmap(ptr, i64, i32, i32, i32, i64)
declare i32 @munmap(ptr, i64)
declare i32 @mprotect(ptr, i64, i32)
declare i64 @sysconf(i32)
declare i32 @sigaltstack(ptr, ptr)
declare void @burdin_rt_ctx_asm_switch(ptr, ptr)
declare void @burdin_rt_ctx_boot()

; Defined by the main runtime so the stack-overflow handler can read it: the low end of the usable part of the stack
; the thread is running on, or 0 on a thread's own stack.
@burdin_rt_cur_lo = external thread_local global i64

@burdin_rt_tls_worker = internal thread_local global ptr null
@burdin_rt_spin = internal global i32 0
@burdin_rt_global = internal global ptr null
@burdin_rt_config = internal global i64 0

@msg_thread = private constant [32 x i8] c"panic: could not start a thread\0A"
@msg_stack = private constant [39 x i8] c"panic: could not allocate a task stack\0A"

define internal void @burdin_rt_task_die(ptr %msg, i64 %len) noreturn {
  %w = call i64 @write(i32 2, ptr %msg, i64 %len)
  call void @abort()
  unreachable
}

; ---- threads ----------------------------------------------------------------------------------------------------

define ptr @burdin_rt_thread_main(ptr %arg) {
  ; An alternate signal stack, so a stack overflow on this thread still reaches the handler.
  %alt = call ptr @malloc(i64 65536)
  %st = alloca [24 x i8], align 8
  store [24 x i8] zeroinitializer, ptr %st
  store ptr %alt, ptr %st
  %size_at = getelementptr i8, ptr %st, i64 @SS_SIZE@
  store i64 65536, ptr %size_at
  %r = call i32 @sigaltstack(ptr %st, ptr null)
  %code = load ptr, ptr %arg
  %envp = getelementptr i8, ptr %arg, i64 8
  %env = load ptr, ptr %envp
  call void @free(ptr %arg)
  call void %code(ptr %env, i1 true)
  ret ptr null
}

define void @burdin_rt_thread_start(ptr %code, ptr %env) {
  %e = call ptr @malloc(i64 16)
  store ptr %code, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  store ptr %env, ptr %envp
  %tid = alloca i64, align 8
  %r = call i32 @pthread_create(ptr %tid, ptr null, ptr @burdin_rt_thread_main, ptr %e)
  %bad = icmp ne i32 %r, 0
  br i1 %bad, label %fail, label %ok
fail:
  call void @burdin_rt_task_die(ptr @msg_thread, i64 32)
  unreachable
ok:
  %t = load i64, ptr %tid
  %d = call i32 @pthread_detach(i64 %t)
  ret void
}

define void @burdin_rt_thread_yield() {
  %r = call i32 @sched_yield()
  ret void
}

define i64 @burdin_rt_cpu_count() {
  %n = call i64 @sysconf(i32 @SC_NPROC@)
  %small = icmp slt i64 %n, 1
  %v = select i1 %small, i64 1, i64 %n
  ret i64 %v
}

; ---- locks ------------------------------------------------------------------------------------------------------

define ptr @burdin_rt_mutex_new() {
  %m = call ptr @calloc(i64 1, i64 128)
  %r = call i32 @pthread_mutex_init(ptr %m, ptr null)
  ret ptr %m
}
define void @burdin_rt_mutex_lock(ptr %m) {
  %r = call i32 @pthread_mutex_lock(ptr %m)
  ret void
}
define void @burdin_rt_mutex_unlock(ptr %m) {
  %r = call i32 @pthread_mutex_unlock(ptr %m)
  ret void
}
define void @burdin_rt_mutex_free(ptr %m) {
  %r = call i32 @pthread_mutex_destroy(ptr %m)
  call void @free(ptr %m)
  ret void
}
define ptr @burdin_rt_cond_new() {
  %c = call ptr @calloc(i64 1, i64 128)
  %r = call i32 @pthread_cond_init(ptr %c, ptr null)
  ret ptr %c
}
define void @burdin_rt_cond_wait(ptr %c, ptr %m) {
  %r = call i32 @pthread_cond_wait(ptr %c, ptr %m)
  ret void
}
define void @burdin_rt_cond_signal(ptr %c) {
  %r = call i32 @pthread_cond_signal(ptr %c)
  ret void
}
define void @burdin_rt_cond_broadcast(ptr %c) {
  %r = call i32 @pthread_cond_broadcast(ptr %c)
  ret void
}
define void @burdin_rt_cond_free(ptr %c) {
  %r = call i32 @pthread_cond_destroy(ptr %c)
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
  %y = call i32 @sched_yield()
  br label %try
done:
  ret void
}
define void @burdin_rt_spin_unlock() {
  store atomic i32 0, ptr @burdin_rt_spin release, align 4
  ret void
}

; ---- execution contexts -------------------------------------------------------------------------------------------
; A context is { sp, lo, map, map_size }: the saved stack pointer, the low end of the usable stack, and the mapping.
; Each task stack is `size` usable bytes above a 64 KiB guard region that is not accessible, so running off the end
; faults instead of corrupting a neighbour. The pages are committed by the system as they are touched.

define void @burdin_rt_ctx_entry(ptr %e) {
  %code = load ptr, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  %env = load ptr, ptr %envp
  call void @free(ptr %e)
  call void %code(ptr %env, i1 true)
  ; The body is done. By the contract with the scheduler (spec/tasks.md, TK2) the first word of the worker record the
  ; thread's slot points at is the worker's own context; control goes back there and never returns. The worker is read
  ; again here because the task may have moved to another thread since it started.
  %scratch = alloca [32 x i8], align 8
  %w = call ptr @burdin_rt_tls_get()
  %main = load ptr, ptr %w
  call void @burdin_rt_ctx_switch(ptr %scratch, ptr %main)
  call void @abort()
  unreachable
}

define ptr @burdin_rt_ctx_main() {
  %c = call ptr @calloc(i64 1, i64 32)
  ret ptr %c
}

define ptr @burdin_rt_ctx_new(i64 %size, ptr %code, ptr %env) {
entry:
  %total = add i64 %size, 65536
  %base = call ptr @mmap(ptr null, i64 %total, i32 3, i32 @MAP_FLAGS@, i32 -1, i64 0)
  %bad = icmp eq ptr %base, inttoptr (i64 -1 to ptr)
  br i1 %bad, label %fail, label %ok
fail:
  call void @burdin_rt_task_die(ptr @msg_stack, i64 39)
  unreachable
ok:
  %g = call i32 @mprotect(ptr %base, i64 65536, i32 0)
  %lo = getelementptr i8, ptr %base, i64 65536
  %top = getelementptr i8, ptr %base, i64 %total
  %e = call ptr @malloc(i64 16)
  store ptr %code, ptr %e
  %envp = getelementptr i8, ptr %e, i64 8
  store ptr %env, ptr %envp
  %ctx = call ptr @calloc(i64 1, i64 32)
@INIT_FRAME@
  store ptr %sp, ptr %ctx
  %lo_at = getelementptr i8, ptr %ctx, i64 8
  store ptr %lo, ptr %lo_at
  %map_at = getelementptr i8, ptr %ctx, i64 16
  store ptr %base, ptr %map_at
  %size_at = getelementptr i8, ptr %ctx, i64 24
  store i64 %total, ptr %size_at
  ret ptr %ctx
}

define void @burdin_rt_ctx_switch(ptr %from, ptr %to) noinline {
  %lo_at = getelementptr i8, ptr %to, i64 8
  %lo = load ptr, ptr %lo_at
  %lo_int = ptrtoint ptr %lo to i64
  store volatile i64 %lo_int, ptr @burdin_rt_cur_lo
  call void @burdin_rt_ctx_asm_switch(ptr %from, ptr %to)
  ret void
}

define void @burdin_rt_ctx_free(ptr %ctx) {
  %map_at = getelementptr i8, ptr %ctx, i64 16
  %map = load ptr, ptr %map_at
  %none = icmp eq ptr %map, null
  br i1 %none, label %done, label %unmap
unmap:
  %size_at = getelementptr i8, ptr %ctx, i64 24
  %size = load i64, ptr %size_at
  %r = call i32 @munmap(ptr %map, i64 %size)
  br label %done
done:
  call void @free(ptr %ctx)
  ret void
}
