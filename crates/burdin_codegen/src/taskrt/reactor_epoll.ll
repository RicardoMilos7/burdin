; The reactor for Linux (spec/io.md, IO2): epoll for readiness, an eventfd for wakeups, and a small helper pool
; for calls that block. The scheduler in `std/src/reactor.bd` owns the timers and wakes the waiting tasks; this
; file only reports which waits fired.
;
; A wait record (`__Wait` in `std/src/reactor.bd`) is read through two fixed offsets:
;   0  next: ptr        the runtime links fired waits through this word
;   24 kind: i32        1 = ready, 2 = completed (written by the runtime)
; A record is returned at most once: the fd registration is one-shot and a completion is taken out of its list.

declare ptr @calloc(i64, i64)
declare void @free(ptr)
declare void @abort()
declare i64 @write(i32, ptr, i64)
declare i64 @read(i32, ptr, i64)
declare i32 @epoll_create1(i32)
declare i32 @epoll_ctl(i32, i32, i32, ptr)
declare i32 @epoll_wait(i32, ptr, i32, i32)
declare i32 @eventfd(i32, i32)
declare i32 @pthread_create(ptr, ptr, ptr, ptr)
declare i32 @pthread_detach(i64)
declare i64 @sysconf(i32)
declare ptr @pthread_mutex_init(ptr, ptr)
declare i32 @pthread_mutex_lock(ptr)
declare i32 @pthread_mutex_unlock(ptr)
declare i32 @pthread_cond_init(ptr, ptr)
declare i32 @pthread_cond_wait(ptr, ptr)
declare i32 @pthread_cond_signal(ptr)

%Reactor = type {
  i32,    ; epoll descriptor
  i32,    ; eventfd that wakes the poll
  ptr,    ; job-queue mutex
  ptr,    ; job-queue condition variable
  ptr,    ; job queue head
  ptr,    ; job queue tail
  i32,    ; whether the helper threads were started
  ptr,    ; completion-list mutex
  ptr     ; completion-list head
}

%Job = type {
  ptr,    ; next job
  ptr,    ; the closure's code
  ptr,    ; the closure's record
  ptr,    ; the wait record to complete
  ptr     ; the reactor to wake
}

; ---- creation ---------------------------------------------------------------------------------------------------

define ptr @burdin_rt_io_new() {
entry:
  %r = call ptr @calloc(i64 1, i64 64)
  %epfd = call i32 @epoll_create1(i32 0)
  %bad = icmp slt i32 %epfd, 0
  br i1 %bad, label %fail, label %ok
fail:
  call void @abort()
  unreachable
ok:
  %wake = call i32 @eventfd(i32 0, i32 2048)
  %badw = icmp slt i32 %wake, 0
  br i1 %badw, label %fail, label %created
created:
  %at0 = getelementptr %Reactor, ptr %r, i32 0, i32 0
  store i32 %epfd, ptr %at0
  %at1 = getelementptr %Reactor, ptr %r, i32 0, i32 1
  store i32 %wake, ptr %at1
  %lockp = call ptr @calloc(i64 1, i64 128)
  %cvp = call ptr @calloc(i64 1, i64 128)
  %clp = call ptr @calloc(i64 1, i64 128)
  %mi = call i32 @pthread_mutex_init(ptr %lockp, ptr null)
  %ci = call i32 @pthread_cond_init(ptr %cvp, ptr null)
  %cmi = call i32 @pthread_mutex_init(ptr %clp, ptr null)
  %at2 = getelementptr %Reactor, ptr %r, i32 0, i32 2
  store ptr %lockp, ptr %at2
  %at3 = getelementptr %Reactor, ptr %r, i32 0, i32 3
  store ptr %cvp, ptr %at3
  %at4 = getelementptr %Reactor, ptr %r, i32 0, i32 4
  store ptr null, ptr %at4
  %at5 = getelementptr %Reactor, ptr %r, i32 0, i32 5
  store ptr null, ptr %at5
  %at6 = getelementptr %Reactor, ptr %r, i32 0, i32 6
  store i32 0, ptr %at6
  %at7 = getelementptr %Reactor, ptr %r, i32 0, i32 7
  store ptr %clp, ptr %at7
  %at8 = getelementptr %Reactor, ptr %r, i32 0, i32 8
  store ptr null, ptr %at8
  ; The wake descriptor is part of the epoll set, with a null token: an event with no wait is just a wakeup.
  %ev = alloca [12 x i8], align 4
  store i32 1, ptr %ev
  %datap = getelementptr i8, ptr %ev, i64 4
  store ptr null, ptr %datap
  %reg = call i32 @epoll_ctl(i32 %epfd, i32 1, i32 %wake, ptr %ev)
  ret ptr %r
}

; ---- registration ------------------------------------------------------------------------------------------------

define i32 @burdin_rt_io_watch(ptr %io, i64 %fd, i32 %events, ptr %wait) {
entry:
  %epfd_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %epfd = load i32, ptr %epfd_at
  %fd32 = trunc i64 %fd to i32
  ; EPOLLONESHOT (1 << 30) keeps a ready descriptor from being reported again before the task re-arms it.
  %read = and i32 %events, 1
  %write = and i32 %events, 2
  %wbit = shl i32 %write, 1
  %bits = or i32 %read, %wbit
  %oneshot = or i32 %bits, 1073741824
  %ev = alloca [12 x i8], align 4
  store i32 %oneshot, ptr %ev
  %datap = getelementptr i8, ptr %ev, i64 4
  store ptr %wait, ptr %datap
  %r = call i32 @epoll_ctl(i32 %epfd, i32 1, i32 %fd32, ptr %ev)
  %ok = icmp eq i32 %r, 0
  br i1 %ok, label %done, label %exists
exists:
  ; A descriptor that is still registered (a stale registration the caller re-arms) is updated instead.
  %r2 = call i32 @epoll_ctl(i32 %epfd, i32 3, i32 %fd32, ptr %ev)
  br label %done
done:
  %last = phi i32 [ 0, %exists ], [ 0, %entry ]
  ret i32 %last
}

define void @burdin_rt_io_unwatch(ptr %io, i64 %fd) {
entry:
  %epfd_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %epfd = load i32, ptr %epfd_at
  %fd32 = trunc i64 %fd to i32
  %r = call i32 @epoll_ctl(i32 %epfd, i32 2, i32 %fd32, ptr null)
  ret void
}

; ---- waiting ----------------------------------------------------------------------------------------------------

define ptr @burdin_rt_io_poll(ptr %io, i64 %timeout_ms) {
entry:
  %epfd_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %epfd = load i32, ptr %epfd_at
  %wake_at = getelementptr %Reactor, ptr %io, i32 0, i32 1
  %wake = load i32, ptr %wake_at
  %events = alloca [768 x i8], align 8
  %timeout = trunc i64 %timeout_ms to i32
  %n = call i32 @epoll_wait(i32 %epfd, ptr %events, i32 64, i32 %timeout)
  %head = alloca ptr, align 8
  store ptr null, ptr %head
  %i = alloca i64, align 8
  store i64 0, ptr %i
  br label %loop
loop:
  %i_now = load i64, ptr %i
  %ni = sext i32 %n to i64
  %more = icmp slt i64 %i_now, %ni
  br i1 %more, label %event, label %drain
event:
  %offset = mul i64 %i_now, 12
  %ev = getelementptr i8, ptr %events, i64 %offset
  %datap = getelementptr i8, ptr %ev, i64 4
  %wait = load ptr, ptr %datap
  %is_wake = icmp eq ptr %wait, null
  br i1 %is_wake, label %woken, label %fired
woken:
  ; Drain the eventfd; the value itself means nothing.
  %count = alloca i64, align 8
  %got = call i64 @read(i32 %wake, ptr %count, i64 8)
  br label %next
fired:
  ; Chain the record in front of the ones already collected; its first word is `next`.
  %previous = load ptr, ptr %head
  store ptr %previous, ptr %wait
  %kind = getelementptr i8, ptr %wait, i64 24
  store i32 1, ptr %kind
  store ptr %wait, ptr %head
  br label %next
next:
  %i_next = add i64 %i_now, 1
  store i64 %i_next, ptr %i
  br label %loop
drain:
  ; Move every finished offloaded call into the result list.
  %clp = getelementptr %Reactor, ptr %io, i32 0, i32 7
  %clock = load ptr, ptr %clp
  %lr = call i32 @pthread_mutex_lock(ptr %clock)
  br label %drain_loop
drain_loop:
  %c_at = getelementptr %Reactor, ptr %io, i32 0, i32 8
  %c = load ptr, ptr %c_at
  %none = icmp eq ptr %c, null
  br i1 %none, label %drain_done, label %take
take:
  %cnext_at = getelementptr i8, ptr %c, i64 0
  %cnext = load ptr, ptr %cnext_at
  store ptr %cnext, ptr %c_at
  %old = load ptr, ptr %head
  store ptr %old, ptr %cnext_at
  store ptr %c, ptr %head
  %ckind = getelementptr i8, ptr %c, i64 24
  store i32 2, ptr %ckind
  br label %drain_loop
drain_done:
  %ur = call i32 @pthread_mutex_unlock(ptr %clock)
  %result = load ptr, ptr %head
  ret ptr %result
}

define void @burdin_rt_io_wake(ptr %io) {
entry:
  %wake_at = getelementptr %Reactor, ptr %io, i32 0, i32 1
  %wake = load i32, ptr %wake_at
  %one = alloca i64, align 8
  store i64 1, ptr %one
  %r = call i64 @write(i32 %wake, ptr %one, i64 8)
  ret void
}

; ---- the helper pool ---------------------------------------------------------------------------------------------
; Jobs run on a few threads so a call that blocks (a file operation, a `blocking` extern) never stalls a scheduler
; worker. The waiting task parks on its own waiter; the completion list is what the reactor reports.

define void @burdin_rt_pool_submit(ptr %io, ptr %wait, ptr %code, ptr %env) {
entry:
  %job = call ptr @calloc(i64 1, i64 40)
  %codep = getelementptr i8, ptr %job, i64 8
  store ptr %code, ptr %codep
  %envp = getelementptr i8, ptr %job, i64 16
  store ptr %env, ptr %envp
  %waitp = getelementptr i8, ptr %job, i64 24
  store ptr %wait, ptr %waitp
  %iop = getelementptr i8, ptr %job, i64 32
  store ptr %io, ptr %iop
  %lock_at = getelementptr %Reactor, ptr %io, i32 0, i32 2
  %lock = load ptr, ptr %lock_at
  %lr = call i32 @pthread_mutex_lock(ptr %lock)
  ; Start the threads the first time.
  %started_at = getelementptr %Reactor, ptr %io, i32 0, i32 6
  %started = load i32, ptr %started_at
  %need = icmp eq i32 %started, 0
  br i1 %need, label %start, label %enqueue
start:
  store i32 1, ptr %started_at
  %ncpu = call i64 @sysconf(i32 84)
  %small = icmp slt i64 %ncpu, 2
  %two = select i1 %small, i64 2, i64 %ncpu
  %big = icmp sgt i64 %two, 4
  %n = select i1 %big, i64 4, i64 %two
  %i = alloca i64, align 8
  store i64 0, ptr %i
  br label %spawn_loop
spawn_loop:
  %k = load i64, ptr %i
  %more = icmp slt i64 %k, %n
  br i1 %more, label %spawn, label %enqueue
spawn:
  %tid = alloca i64, align 8
  %cr = call i32 @pthread_create(ptr %tid, ptr null, ptr @burdin_rt_pool_main, ptr %io)
  %t = load i64, ptr %tid
  %dr = call i32 @pthread_detach(i64 %t)
  %kn = add i64 %k, 1
  store i64 %kn, ptr %i
  br label %spawn_loop
enqueue:
  ; Append to the tail.
  %tail_at = getelementptr %Reactor, ptr %io, i32 0, i32 5
  %tail = load ptr, ptr %tail_at
  %empty = icmp eq ptr %tail, null
  br i1 %empty, label %first, label %link
first:
  %head_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  store ptr %job, ptr %head_at
  br label %linked
link:
  store ptr %job, ptr %tail
  br label %linked
linked:
  store ptr %job, ptr %tail_at
  %cv_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %cv = load ptr, ptr %cv_at
  %sr = call i32 @pthread_cond_signal(ptr %cv)
  %ur = call i32 @pthread_mutex_unlock(ptr %lock)
  ret void
}

define ptr @burdin_rt_pool_main(ptr %io) {
entry:
  br label %loop
loop:
  %lock_at = getelementptr %Reactor, ptr %io, i32 0, i32 2
  %lock = load ptr, ptr %lock_at
  %lr = call i32 @pthread_mutex_lock(ptr %lock)
  br label %check
check:
  %head_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  %job = load ptr, ptr %head_at
  %none = icmp eq ptr %job, null
  br i1 %none, label %idle, label %take
idle:
  %cv_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %cv = load ptr, ptr %cv_at
  %wr = call i32 @pthread_cond_wait(ptr %cv, ptr %lock)
  br label %check
take:
  %next_at = getelementptr i8, ptr %job, i64 0
  %next = load ptr, ptr %next_at
  store ptr %next, ptr %head_at
  %now_empty = icmp eq ptr %next, null
  br i1 %now_empty, label %clear_tail, label %unlocked
clear_tail:
  %tail_at = getelementptr %Reactor, ptr %io, i32 0, i32 5
  store ptr null, ptr %tail_at
  br label %unlocked
unlocked:
  %ur = call i32 @pthread_mutex_unlock(ptr %lock)
  ; Run the job.
  %code_at = getelementptr i8, ptr %job, i64 8
  %code = load ptr, ptr %code_at
  %env_at = getelementptr i8, ptr %job, i64 16
  %env = load ptr, ptr %env_at
  call void %code(ptr %env, i1 true)
  ; Publish the completion.
  %wait_at = getelementptr i8, ptr %job, i64 24
  %wait = load ptr, ptr %wait_at
  %clp = getelementptr %Reactor, ptr %io, i32 0, i32 7
  %clock = load ptr, ptr %clp
  %clr = call i32 @pthread_mutex_lock(ptr %clock)
  %chead_at = getelementptr %Reactor, ptr %io, i32 0, i32 8
  %chead = load ptr, ptr %chead_at
  store ptr %chead, ptr %wait
  store ptr %wait, ptr %chead_at
  %cur = call i32 @pthread_mutex_unlock(ptr %clock)
  call void @burdin_rt_io_wake(ptr %io)
  call void @free(ptr %job)
  br label %loop
}
