; The reactor for macOS (spec/io.md, IO2): kqueue for readiness and a `EVFILT_USER` event for wakeups, plus the
; same helper pool as the other platforms. A wait record (`Wait` in `std/src/reactor.bd`) is read through two
; fixed offsets: 0 is `next` (the runtime's result chain) and 24 is `kind` (1 ready, 2 completed).
;
; A `struct kevent` on 64-bit Darwin is 32 bytes: ident (i64) at 0, filter (i16) at 8, flags (u16) at 10,
; fflags (u32) at 12, data (i64) at 16, and udata (ptr) at 24.

declare ptr @calloc(i64, i64)
declare void @free(ptr)
declare void @abort()
declare i32 @kqueue()
declare i32 @kevent(i32, ptr, i32, ptr, i32, ptr)
declare i32 @pthread_create(ptr, ptr, ptr, ptr)
declare i32 @pthread_detach(i64)
declare i64 @sysconf(i32)
declare ptr @pthread_mutex_init(ptr, ptr)
declare i32 @pthread_mutex_lock(ptr)
declare i32 @pthread_mutex_unlock(ptr)
declare i32 @pthread_cond_init(ptr, ptr)
declare i32 @pthread_cond_wait(ptr, ptr)
declare i32 @pthread_cond_signal(ptr)
declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)

; EVFILT_READ = -1, EVFILT_WRITE = -2, EVFILT_USER = -10; EV_ADD = 1, EV_CLEAR = 0x20, EV_ONESHOT = 0x10.
; NOTE_TRIGGER = 0x01000000; _SC_NPROCESSORS_ONLN = 58 on Darwin.

%Reactor = type {
  i32,    ; kqueue descriptor
  i32,    ; unused (the wakeup is a user event on the same queue)
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

define ptr @burdin_rt_io_new() {
entry:
  %r = call ptr @calloc(i64 1, i64 64)
  %kq = call i32 @kqueue()
  %bad = icmp slt i32 %kq, 0
  br i1 %bad, label %fail, label %ok
fail:
  call void @abort()
  unreachable
ok:
  %at0 = getelementptr %Reactor, ptr %r, i32 0, i32 0
  store i32 %kq, ptr %at0
  %at1 = getelementptr %Reactor, ptr %r, i32 0, i32 1
  store i32 0, ptr %at1
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
  ; Register the user event that `io_wake` triggers.
  %change = alloca [32 x i8], align 8
  call void @llvm.memset.p0.i64(ptr %change, i8 0, i64 32, i1 false)
  store i64 1, ptr %change
  %filter = getelementptr i8, ptr %change, i64 8
  store i16 -10, ptr %filter
  %flags = getelementptr i8, ptr %change, i64 10
  store i16 33, ptr %flags
  %reg = call i32 @kevent(i32 %kq, ptr %change, i32 1, ptr null, i32 0, ptr null)
  ret ptr %r
}

define i32 @burdin_rt_io_watch(ptr %io, i64 %fd, i32 %events, ptr %wait) {
entry:
  %kq_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %kq = load i32, ptr %kq_at
  %change = alloca [32 x i8], align 8
  call void @llvm.memset.p0.i64(ptr %change, i8 0, i64 32, i1 false)
  store i64 %fd, ptr %change
  %want_read = and i32 %events, 1
  %read = icmp ne i32 %want_read, 0
  %filter = select i1 %read, i16 -1, i16 -2
  %filter_at = getelementptr i8, ptr %change, i64 8
  store i16 %filter, ptr %filter_at
  ; EV_ADD | EV_ONESHOT: the registration fires once and is gone.
  %flags_at = getelementptr i8, ptr %change, i64 10
  store i16 17, ptr %flags_at
  %udata = getelementptr i8, ptr %change, i64 24
  store ptr %wait, ptr %udata
  %r = call i32 @kevent(i32 %kq, ptr %change, i32 1, ptr null, i32 0, ptr null)
  %ok = icmp eq i32 %r, 0
  %ret = select i1 %ok, i32 0, i32 -1
  ret i32 %ret
}

define void @burdin_rt_io_unwatch(ptr %io, i64 %fd) {
entry:
  %kq_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %kq = load i32, ptr %kq_at
  %changes = alloca [64 x i8], align 8
  call void @llvm.memset.p0.i64(ptr %changes, i8 0, i64 64, i1 false)
  store i64 %fd, ptr %changes
  %f1 = getelementptr i8, ptr %changes, i64 8
  store i16 -1, ptr %f1
  %g1 = getelementptr i8, ptr %changes, i64 10
  store i16 2, ptr %g1
  %second = getelementptr i8, ptr %changes, i64 32
  store i64 %fd, ptr %second
  %f2 = getelementptr i8, ptr %second, i64 8
  store i16 -2, ptr %f2
  %g2 = getelementptr i8, ptr %second, i64 10
  store i16 2, ptr %g2
  %r = call i32 @kevent(i32 %kq, ptr %changes, i32 2, ptr null, i32 0, ptr null)
  ret void
}

define ptr @burdin_rt_io_poll(ptr %io, i64 %timeout_ms) {
entry:
  %kq_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %kq = load i32, ptr %kq_at
  %events = alloca [2048 x i8], align 8
  %ts = alloca [16 x i8], align 8
  %infinite = icmp slt i64 %timeout_ms, 0
  %sec = sdiv i64 %timeout_ms, 1000
  %rem = srem i64 %timeout_ms, 1000
  %nsec = mul i64 %rem, 1000000
  %sec_at = getelementptr i8, ptr %ts, i64 0
  store i64 %sec, ptr %sec_at
  %nsec_at = getelementptr i8, ptr %ts, i64 8
  store i64 %nsec, ptr %nsec_at
  %tsp = select i1 %infinite, ptr null, ptr %ts
  %n = call i32 @kevent(i32 %kq, ptr null, i32 0, ptr %events, i32 64, ptr %tsp)
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
  %offset = mul i64 %i_now, 32
  %ev = getelementptr i8, ptr %events, i64 %offset
  %filter_at = getelementptr i8, ptr %ev, i64 8
  %filter = load i16, ptr %filter_at
  %is_user = icmp eq i16 %filter, -10
  br i1 %is_user, label %next, label %fired
fired:
  %udata = getelementptr i8, ptr %ev, i64 24
  %wait = load ptr, ptr %udata
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
  %kq_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %kq = load i32, ptr %kq_at
  %change = alloca [32 x i8], align 8
  call void @llvm.memset.p0.i64(ptr %change, i8 0, i64 32, i1 false)
  store i64 1, ptr %change
  %filter = getelementptr i8, ptr %change, i64 8
  store i16 -10, ptr %filter
  %fflags = getelementptr i8, ptr %change, i64 12
  store i32 16777216, ptr %fflags
  %r = call i32 @kevent(i32 %kq, ptr %change, i32 1, ptr null, i32 0, ptr null)
  ret void
}

; ---- the helper pool (identical to the other platforms) ----------------------------------------------------------

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
  %started_at = getelementptr %Reactor, ptr %io, i32 0, i32 6
  %started = load i32, ptr %started_at
  %need = icmp eq i32 %started, 0
  br i1 %need, label %start, label %enqueue
start:
  store i32 1, ptr %started_at
  %ncpu = call i64 @sysconf(i32 58)
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
  %code_at = getelementptr i8, ptr %job, i64 8
  %code = load ptr, ptr %code_at
  %env_at = getelementptr i8, ptr %job, i64 16
  %env = load ptr, ptr %env_at
  call void %code(ptr %env, i1 true)
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
