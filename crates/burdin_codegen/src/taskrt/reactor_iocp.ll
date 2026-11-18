; The reactor for Windows (spec/io.md, IO2): an I/O completion port carries the completed helper calls, Winsock
; `WSAPoll` reports socket readiness, and a loopback UDP datagram wakes the poll. A wait record (`__Wait` in
; `std/src/reactor.bd`) is read through two fixed offsets: 0 is `next` (the runtime's result chain) and 24 is
; `kind` (1 ready, 2 completed).
;
; Socket descriptors and handles are `i64`: `SOCKET` is pointer-sized on 64-bit Windows.

declare ptr @calloc(i64, i64)
declare void @free(ptr)
declare void @abort()
declare i64 @CreateIoCompletionPort(i64, i64, i64, i32)
declare i32 @PostQueuedCompletionStatus(i64, i32, i64, ptr)
declare i32 @GetQueuedCompletionStatus(i64, ptr, ptr, ptr, i32)
declare i64 @WSASocketA(i32, i32, i32, ptr, i32, i32)
declare i32 @WSAPoll(ptr, i32, i32)
declare i32 @bind(i64, ptr, i32)
declare i32 @getsockname(i64, ptr, ptr)
declare i64 @sendto(i64, ptr, i64, i32, ptr, i32)
declare i64 @recv(i64, ptr, i64, i32)
declare ptr @CreateThread(ptr, i64, ptr, ptr, i32, ptr)
declare i32 @CloseHandle(ptr)
declare i32 @WSAStartup(i32, ptr)
declare void @InitializeSRWLock(ptr)
declare void @AcquireSRWLockExclusive(ptr)
declare void @ReleaseSRWLockExclusive(ptr)
declare void @InitializeConditionVariable(ptr)
declare i32 @SleepConditionVariableSRW(ptr, ptr, i32, i32)
declare void @WakeConditionVariable(ptr)
declare i32 @GetActiveProcessorCount(i16)
declare i32 @ioctlsocket(i64, i32, ptr)

; WSAPoll's event bits and the table's capacity.
; POLLRDNORM = 0x0100, POLLWRNORM = 0x0010.
; A `WSAPOLLFD` is 16 bytes: SOCKET fd, SHORT events, SHORT revents, padding.
%Reactor = type {
  i64,    ; completion port
  i64,    ; wake socket (the one `WSAPoll` waits on)
  ptr,    ; wake address (`sockaddr_in`, 16 bytes)
  ptr,    ; readiness table (`%Entry`)
  ptr,    ; table lock (SRW)
  i32,    ; entry count
  ptr,    ; job-queue lock (SRW)
  ptr,    ; job-queue condition variable
  ptr,    ; job queue head
  ptr,    ; job queue tail
  i32,    ; whether the helper threads were started
  i64     ; sender socket: it wakes the poll without sending from the polled socket
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
  ; Winsock, so a socket call made before `burdin_rt_net_socket` still works.
  %wsa = alloca [512 x i8], align 8
  %ws = call i32 @WSAStartup(i32 514, ptr %wsa)
  %r = call ptr @calloc(i64 1, i64 96)
  ; The completion port: INVALID_HANDLE_VALUE (-1) creates a new one with no file handle.
  %iocp = call i64 @CreateIoCompletionPort(i64 -1, i64 0, i64 0, i32 1)
  %at0 = getelementptr %Reactor, ptr %r, i32 0, i32 0
  store i64 %iocp, ptr %at0
  ; The wake socket: a datagram socket bound to 127.0.0.1:0; `io_wake` sends one byte to it. Sending from the
  ; same socket while another thread waits in `WSAPoll` or `recv` on it deadlocks Winsock, so a second socket is
  ; the sender.
  %wake = call i64 @WSASocketA(i32 2, i32 2, i32 0, ptr null, i32 0, i32 0)
  %sender = call i64 @WSASocketA(i32 2, i32 2, i32 0, ptr null, i32 0, i32 0)
  ; The sender never blocks: a full receive queue means a datagram is already waiting, so the poll will return and
  ; take a fresh set without the one that would have been dropped.
  %nb = alloca i32, align 4
  store i32 1, ptr %nb
  %nbr = call i32 @ioctlsocket(i64 %sender, i32 2147772030, ptr %nb)
  %at11 = getelementptr %Reactor, ptr %r, i32 0, i32 11
  store i64 %sender, ptr %at11
  %addr = call ptr @calloc(i64 1, i64 16)
  store i16 2, ptr %addr, align 2
  %port_at = getelementptr i8, ptr %addr, i64 2
  store i16 0, ptr %port_at, align 2
  %b0 = getelementptr i8, ptr %addr, i64 4
  store i8 127, ptr %b0
  %b1 = getelementptr i8, ptr %addr, i64 5
  store i8 0, ptr %b1
  %b2 = getelementptr i8, ptr %addr, i64 6
  store i8 0, ptr %b2
  %b3 = getelementptr i8, ptr %addr, i64 7
  store i8 1, ptr %b3
  %br = call i32 @bind(i64 %wake, ptr %addr, i32 16)
  %bad = icmp slt i32 %br, 0
  br i1 %bad, label %fail, label %ok
fail:
  call void @abort()
  unreachable
ok:
  %length = alloca i32, align 4
  store i32 16, ptr %length
  %gr = call i32 @getsockname(i64 %wake, ptr %addr, ptr %length)
  %at1 = getelementptr %Reactor, ptr %r, i32 0, i32 1
  store i64 %wake, ptr %at1
  %at2 = getelementptr %Reactor, ptr %r, i32 0, i32 2
  store ptr %addr, ptr %at2
  %table = call ptr @calloc(i64 256, i64 24)
  %at3 = getelementptr %Reactor, ptr %r, i32 0, i32 3
  store ptr %table, ptr %at3
  %tlock = call ptr @calloc(i64 1, i64 16)
  call void @InitializeSRWLock(ptr %tlock)
  %at4 = getelementptr %Reactor, ptr %r, i32 0, i32 4
  store ptr %tlock, ptr %at4
  %at5 = getelementptr %Reactor, ptr %r, i32 0, i32 5
  store i32 0, ptr %at5
  %jlock = call ptr @calloc(i64 1, i64 16)
  call void @InitializeSRWLock(ptr %jlock)
  %jcv = call ptr @calloc(i64 1, i64 16)
  call void @InitializeConditionVariable(ptr %jcv)
  %at6 = getelementptr %Reactor, ptr %r, i32 0, i32 6
  store ptr %jlock, ptr %at6
  %at7 = getelementptr %Reactor, ptr %r, i32 0, i32 7
  store ptr %jcv, ptr %at7
  %at8 = getelementptr %Reactor, ptr %r, i32 0, i32 8
  store ptr null, ptr %at8
  %at9 = getelementptr %Reactor, ptr %r, i32 0, i32 9
  store ptr null, ptr %at9
  %at10 = getelementptr %Reactor, ptr %r, i32 0, i32 10
  store i32 0, ptr %at10
  ret ptr %r
}

define i32 @burdin_rt_io_watch(ptr %io, i64 %fd, i32 %events, ptr %wait) {
entry:
  %tlock_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  %tlock = load ptr, ptr %tlock_at
  call void @AcquireSRWLockExclusive(ptr %tlock)
  %table_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %table = load ptr, ptr %table_at
  %found = alloca ptr, align 8
  store ptr null, ptr %found
  %free = alloca ptr, align 8
  store ptr null, ptr %free
  %i = alloca i64, align 8
  store i64 0, ptr %i
  br label %scan
scan:
  %k = load i64, ptr %i
  %more = icmp slt i64 %k, 256
  br i1 %more, label %look, label %scanned
look:
  %offset = mul i64 %k, 24
  %row = getelementptr i8, ptr %table, i64 %offset
  %entry_fd = load i64, ptr %row
  %same = icmp eq i64 %entry_fd, %fd
  br i1 %same, label %hit, label %maybe_free
maybe_free:
  %is_free = icmp eq i64 %entry_fd, 0
  %no_free = load ptr, ptr %free
  %still_none = icmp eq ptr %no_free, null
  %take_free = and i1 %is_free, %still_none
  br i1 %take_free, label %remember, label %advance
remember:
  store ptr %row, ptr %free
  br label %advance
hit:
  store ptr %row, ptr %found
  br label %scanned
advance:
  %kn = add i64 %k, 1
  store i64 %kn, ptr %i
  br label %scan
scanned:
  %slot = load ptr, ptr %found
  %missing = icmp eq ptr %slot, null
  %fresh = load ptr, ptr %free
  %chosen = select i1 %missing, ptr %fresh, ptr %slot
  %none = icmp eq ptr %chosen, null
  br i1 %none, label %full, label %store
store:
  store i64 %fd, ptr %chosen
  %events_at = getelementptr i8, ptr %chosen, i64 8
  %wide = sext i32 %events to i64
  store i64 %wide, ptr %events_at
  %wait_at = getelementptr i8, ptr %chosen, i64 16
  store ptr %wait, ptr %wait_at
  ; Count a fresh entry.
  %was_new = icmp eq ptr %slot, null
  br i1 %was_new, label %count, label %unlock
count:
  %count_at = getelementptr %Reactor, ptr %io, i32 0, i32 5
  %n = load i32, ptr %count_at
  %n1 = add i32 %n, 1
  store i32 %n1, ptr %count_at
  br label %unlock
full:
  call void @ReleaseSRWLockExclusive(ptr %tlock)
  ret i32 -1
unlock:
  call void @ReleaseSRWLockExclusive(ptr %tlock)
  ret i32 0
}

define void @burdin_rt_io_unwatch(ptr %io, i64 %fd) {
entry:
  %tlock_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  %tlock = load ptr, ptr %tlock_at
  call void @AcquireSRWLockExclusive(ptr %tlock)
  %table_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %table = load ptr, ptr %table_at
  %i = alloca i64, align 8
  store i64 0, ptr %i
  br label %scan
scan:
  %k = load i64, ptr %i
  %more = icmp slt i64 %k, 256
  br i1 %more, label %look, label %done
look:
  %offset = mul i64 %k, 24
  %row = getelementptr i8, ptr %table, i64 %offset
  %entry_fd = load i64, ptr %row
  %same = icmp eq i64 %entry_fd, %fd
  br i1 %same, label %clear, label %advance
clear:
  store i64 0, ptr %row
  %count_at = getelementptr %Reactor, ptr %io, i32 0, i32 5
  %n = load i32, ptr %count_at
  %n1 = sub i32 %n, 1
  store i32 %n1, ptr %count_at
  br label %done
advance:
  %kn = add i64 %k, 1
  store i64 %kn, ptr %i
  br label %scan
done:
  call void @ReleaseSRWLockExclusive(ptr %tlock)
  ret void
}

define ptr @burdin_rt_io_poll(ptr %io, i64 %timeout_ms) {
entry:
  %fds = alloca [4112 x i8], align 8
  %count = alloca i64, align 8
  store i64 0, ptr %count
  ; The wake socket is the first entry.
  %wake_at = getelementptr %Reactor, ptr %io, i32 0, i32 1
  %wake = load i64, ptr %wake_at
  store i64 %wake, ptr %fds
  %events_at = getelementptr i8, ptr %fds, i64 8
  store i16 256, ptr %events_at
  %revents_at = getelementptr i8, ptr %fds, i64 10
  store i16 0, ptr %revents_at
  store i64 1, ptr %count
  ; Snapshot the readiness table.
  %tlock_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  %tlock = load ptr, ptr %tlock_at
  call void @AcquireSRWLockExclusive(ptr %tlock)
  %table_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %table = load ptr, ptr %table_at
  %i = alloca i64, align 8
  store i64 0, ptr %i
  br label %scan
scan:
  %k = load i64, ptr %i
  %more = icmp slt i64 %k, 256
  br i1 %more, label %look, label %scanned
look:
  %offset = mul i64 %k, 24
  %row = getelementptr i8, ptr %table, i64 %offset
  %entry_fd = load i64, ptr %row
  %free = icmp eq i64 %entry_fd, 0
  br i1 %free, label %advance, label %copy
copy:
  %n = load i64, ptr %count
  %room = icmp slt i64 %n, 256
  br i1 %room, label %add, label %advance
add:
  %slot_offset = mul i64 %n, 16
  %slot = getelementptr i8, ptr %fds, i64 %slot_offset
  store i64 %entry_fd, ptr %slot
  %wanted_at = getelementptr i8, ptr %row, i64 8
  %wanted = load i64, ptr %wanted_at
  %read_bit = and i64 %wanted, 1
  %write_bit = and i64 %wanted, 2
  %read_ev = mul i64 %read_bit, 256
  %write_ev = mul i64 %write_bit, 8
  %ev = or i64 %read_ev, %write_ev
  %ev16 = trunc i64 %ev to i16
  %slot_events = getelementptr i8, ptr %slot, i64 8
  store i16 %ev16, ptr %slot_events
  %slot_revents = getelementptr i8, ptr %slot, i64 10
  store i16 0, ptr %slot_revents
  %n1 = add i64 %n, 1
  store i64 %n1, ptr %count
  br label %advance
advance:
  %kn = add i64 %k, 1
  store i64 %kn, ptr %i
  br label %scan
scanned:
  call void @ReleaseSRWLockExclusive(ptr %tlock)
  %total = load i64, ptr %count
  %total32 = trunc i64 %total to i32
  %timeout = trunc i64 %timeout_ms to i32
  %ready = call i32 @WSAPoll(ptr %fds, i32 %total32, i32 %timeout)
  %head = alloca ptr, align 8
  store ptr null, ptr %head
  ; Walk the ready entries: index 0 is the wake socket, the rest map back to the table.
  %j = alloca i64, align 8
  store i64 0, ptr %j
  br label %collect
collect:
  %m = load i64, ptr %j
  %mt = load i64, ptr %count
  %more2 = icmp slt i64 %m, %mt
  br i1 %more2, label %polled, label %completions
polled:
  %poffset = mul i64 %m, 16
  %pfd = getelementptr i8, ptr %fds, i64 %poffset
  %prevents_at = getelementptr i8, ptr %pfd, i64 10
  %prevents = load i16, ptr %prevents_at
  %was_ready = icmp ne i16 %prevents, 0
  br i1 %was_ready, label %checked, label %next_poll
checked:
  %first = icmp eq i64 %m, 0
  br i1 %first, label %drain_wake, label %claim
drain_wake:
  %byte = alloca [8 x i8], align 8
  %got = call i64 @recv(i64 %wake, ptr %byte, i64 8, i32 0)
  br label %next_poll
claim:
  ; Remove the table entry only if it still belongs to the same wait.
  %pfd_fd = load i64, ptr %pfd
  %claim_lock_at = getelementptr %Reactor, ptr %io, i32 0, i32 4
  %claim_lock = load ptr, ptr %claim_lock_at
  call void @AcquireSRWLockExclusive(ptr %claim_lock)
  %ctable_at = getelementptr %Reactor, ptr %io, i32 0, i32 3
  %ctable = load ptr, ptr %ctable_at
  %ci = alloca i64, align 8
  store i64 0, ptr %ci
  br label %find
find:
  %ck = load i64, ptr %ci
  %cmore = icmp slt i64 %ck, 256
  br i1 %cmore, label %find_look, label %claim_done
find_look:
  %coffset = mul i64 %ck, 24
  %centry = getelementptr i8, ptr %ctable, i64 %coffset
  %cfd = load i64, ptr %centry
  %same = icmp eq i64 %cfd, %pfd_fd
  br i1 %same, label %found, label %find_next
find_next:
  %ckn = add i64 %ck, 1
  store i64 %ckn, ptr %ci
  br label %find
found:
  %cwait_at = getelementptr i8, ptr %centry, i64 16
  %cwait = load ptr, ptr %cwait_at
  store i64 0, ptr %centry
  %count_at = getelementptr %Reactor, ptr %io, i32 0, i32 5
  %cn = load i32, ptr %count_at
  %cn1 = sub i32 %cn, 1
  store i32 %cn1, ptr %count_at
  call void @ReleaseSRWLockExclusive(ptr %claim_lock)
  %old = load ptr, ptr %head
  store ptr %old, ptr %cwait
  %kind = getelementptr i8, ptr %cwait, i64 24
  store i32 1, ptr %kind
  store ptr %cwait, ptr %head
  br label %next_poll
claim_done:
  call void @ReleaseSRWLockExclusive(ptr %claim_lock)
  br label %next_poll
next_poll:
  %mn = add i64 %m, 1
  store i64 %mn, ptr %j
  br label %collect
completions:
  ; Drain the completion port: each entry is a helper call that finished.
  %iocp_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %iocp = load i64, ptr %iocp_at
  %bytes = alloca i32, align 4
  %key = alloca i64, align 8
  %overlapped = alloca ptr, align 8
  store ptr null, ptr %overlapped
  br label %drain_iocp
drain_iocp:
  %ok = call i32 @GetQueuedCompletionStatus(i64 %iocp, ptr %bytes, ptr %key, ptr %overlapped, i32 0)
  %entry_ptr = load ptr, ptr %overlapped
  %none = icmp eq ptr %entry_ptr, null
  br i1 %none, label %done, label %take
take:
  %prev = load ptr, ptr %head
  store ptr %prev, ptr %entry_ptr
  %ckind = getelementptr i8, ptr %entry_ptr, i64 24
  store i32 2, ptr %ckind
  store ptr %entry_ptr, ptr %head
  store ptr null, ptr %overlapped
  br label %drain_iocp
done:
  %result = load ptr, ptr %head
  ret ptr %result
}

define void @burdin_rt_io_wake(ptr %io) {
entry:
  %sender_at = getelementptr %Reactor, ptr %io, i32 0, i32 11
  %sender = load i64, ptr %sender_at
  %addr_at = getelementptr %Reactor, ptr %io, i32 0, i32 2
  %addr = load ptr, ptr %addr_at
  %one = alloca [1 x i8], align 1
  store i8 1, ptr %one
  %r = call i64 @sendto(i64 %sender, ptr %one, i64 1, i32 0, ptr %addr, i32 16)
  ret void
}

; ---- the helper pool ---------------------------------------------------------------------------------------------

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
  %lock_at = getelementptr %Reactor, ptr %io, i32 0, i32 6
  %lock = load ptr, ptr %lock_at
  call void @AcquireSRWLockExclusive(ptr %lock)
  %started_at = getelementptr %Reactor, ptr %io, i32 0, i32 10
  %started = load i32, ptr %started_at
  %need = icmp eq i32 %started, 0
  br i1 %need, label %start, label %enqueue
start:
  store i32 1, ptr %started_at
  %ncpu = call i32 @GetActiveProcessorCount(i16 65535)
  %small = icmp slt i32 %ncpu, 2
  %two = select i1 %small, i32 2, i32 %ncpu
  %big = icmp sgt i32 %two, 4
  %n = select i1 %big, i32 4, i32 %two
  %i = alloca i32, align 4
  store i32 0, ptr %i
  br label %spawn_loop
spawn_loop:
  %k = load i32, ptr %i
  %more = icmp slt i32 %k, %n
  br i1 %more, label %spawn, label %enqueue
spawn:
  %h = call ptr @CreateThread(ptr null, i64 0, ptr @burdin_rt_pool_main, ptr %io, i32 0, ptr null)
  %closed = call i32 @CloseHandle(ptr %h)
  %kn = add i32 %k, 1
  store i32 %kn, ptr %i
  br label %spawn_loop
enqueue:
  %tail_at = getelementptr %Reactor, ptr %io, i32 0, i32 9
  %tail = load ptr, ptr %tail_at
  %empty = icmp eq ptr %tail, null
  br i1 %empty, label %first, label %link
first:
  %head_at = getelementptr %Reactor, ptr %io, i32 0, i32 8
  store ptr %job, ptr %head_at
  br label %linked
link:
  store ptr %job, ptr %tail
  br label %linked
linked:
  store ptr %job, ptr %tail_at
  %cv_at = getelementptr %Reactor, ptr %io, i32 0, i32 7
  %cv = load ptr, ptr %cv_at
  call void @WakeConditionVariable(ptr %cv)
  call void @ReleaseSRWLockExclusive(ptr %lock)
  ret void
}

define i32 @burdin_rt_pool_main(ptr %io) {
entry:
  br label %loop
loop:
  %lock_at = getelementptr %Reactor, ptr %io, i32 0, i32 6
  %lock = load ptr, ptr %lock_at
  call void @AcquireSRWLockExclusive(ptr %lock)
  br label %check
check:
  %head_at = getelementptr %Reactor, ptr %io, i32 0, i32 8
  %job = load ptr, ptr %head_at
  %none = icmp eq ptr %job, null
  br i1 %none, label %idle, label %take
idle:
  %cv_at = getelementptr %Reactor, ptr %io, i32 0, i32 7
  %cv = load ptr, ptr %cv_at
  %wr = call i32 @SleepConditionVariableSRW(ptr %cv, ptr %lock, i32 -1, i32 0)
  br label %check
take:
  %next_at = getelementptr i8, ptr %job, i64 0
  %next = load ptr, ptr %next_at
  store ptr %next, ptr %head_at
  %now_empty = icmp eq ptr %next, null
  br i1 %now_empty, label %clear_tail, label %unlocked
clear_tail:
  %tail_at = getelementptr %Reactor, ptr %io, i32 0, i32 9
  store ptr null, ptr %tail_at
  br label %unlocked
unlocked:
  call void @ReleaseSRWLockExclusive(ptr %lock)
  %code_at = getelementptr i8, ptr %job, i64 8
  %code = load ptr, ptr %code_at
  %env_at = getelementptr i8, ptr %job, i64 16
  %env = load ptr, ptr %env_at
  call void %code(ptr %env, i1 true)
  %wait_at = getelementptr i8, ptr %job, i64 24
  %wait = load ptr, ptr %wait_at
  %iocp_at = getelementptr %Reactor, ptr %io, i32 0, i32 0
  %iocp = load i64, ptr %iocp_at
  %posted = call i32 @PostQueuedCompletionStatus(i64 %iocp, i32 0, i64 0, ptr %wait)
  call void @burdin_rt_io_wake(ptr %io)
  call void @free(ptr %job)
  br label %loop
}
