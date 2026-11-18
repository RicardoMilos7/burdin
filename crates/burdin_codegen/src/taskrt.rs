//! The task runtime's primitives (`spec/tasks.md`, TK2), written as LLVM IR text and linked into the program the first
//! time one is used.
//!
//! `taskrt/unix.ll` and `taskrt/windows.ll` define the same functions: threads, locks, execution contexts, the two
//! slots the scheduler keeps, and the spin lock for one-time initialization. A context switch on Unix is a few
//! instructions of assembly per architecture, added to the module as inline assembly; Windows uses fibers. The
//! scheduler itself is Burdin code in `std/src/task.bd`.
//!
//! The IR is parsed and linked rather than built with the builder because it is long and mostly straight-line calls:
//! as text it reads like the C it stands for.

use crate::emit::GenResult;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::module::{Linkage, Module};

const UNIX_IR: &str = include_str!("taskrt/unix.ll");
const WINDOWS_IR: &str = include_str!("taskrt/windows.ll");
/// The reactor of `spec/io.md` (IO2): epoll on Linux, kqueue on macOS, and a completion port with `WSAPoll` on
/// Windows. Each defines the same `burdin_rt_io_*` functions and the helper pool.
const EPOLL_IR: &str = include_str!("taskrt/reactor_epoll.ll");
const KQUEUE_IR: &str = include_str!("taskrt/reactor_kqueue.ll");
const IOCP_IR: &str = include_str!("taskrt/reactor_iocp.ll");

/// The size of the guard region below a task stack, in bytes. A fault inside it is a stack overflow
/// (`burdin_rt_stack_overflow` compares the fault address with the running stack's low end).
pub(crate) const GUARD: u64 = 65536;

/// Links the task runtime into `module` for the target named by `triple`. Every function it defines becomes
/// internal, so an unused one does not stay in the binary of an optimized build.
pub(crate) fn link<'ctx>(cx: &'ctx Context, module: &Module<'ctx>, triple: &str) -> GenResult<()> {
    let windows = triple.contains("windows");
    let darwin = triple.contains("darwin") || triple.contains("apple");
    let aarch64 = triple.starts_with("aarch64") || triple.starts_with("arm64");
    let mut text = if windows { WINDOWS_IR.to_string() } else { UNIX_IR.to_string() };
    if !windows {
        let (map_flags, ss_size, nproc) = if darwin {
            // MAP_PRIVATE | MAP_ANON | MAP_NORESERVE; `stack_t` is { sp, size, flags }; _SC_NPROCESSORS_ONLN.
            (0x1042u32, 8u32, 58u32)
        } else {
            // MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE | MAP_STACK; { sp, flags, size }; _SC_NPROCESSORS_ONLN.
            (0x24022, 16, 84)
        };
        text = text
            .replace("@MAP_FLAGS@", &map_flags.to_string())
            .replace("@SS_SIZE@", &ss_size.to_string())
            .replace("@SC_NPROC@", &nproc.to_string())
            .replace("@INIT_FRAME@", if aarch64 { INIT_FRAME_AARCH64 } else { INIT_FRAME_X86_64 });
    }
    link_text(cx, module, &text, "burdin_taskrt")?;
    let reactor = if windows {
        IOCP_IR
    } else if darwin {
        KQUEUE_IR
    } else {
        EPOLL_IR
    };
    link_text(cx, module, reactor, "burdin_reactor")?;
    if !windows {
        // The module has no other inline assembly, so setting it does not replace anything.
        module.set_inline_assembly(&switch_asm(darwin, aarch64));
    }
    for function in module.get_functions() {
        if function.count_basic_blocks() > 0 && function.get_name().to_bytes().starts_with(b"burdin_rt_") {
            // Only the task runtime's own definitions are touched: the main runtime's are already internal.
            if function.get_linkage() == Linkage::External {
                function.set_linkage(Linkage::Internal);
            }
        }
    }
    Ok(())
}

/// Parses `text` as a module and links it into `module`.
fn link_text<'ctx>(cx: &'ctx Context, module: &Module<'ctx>, text: &str, name: &str) -> GenResult<()> {
    let buffer = MemoryBuffer::create_from_memory_range_copy(text.as_bytes(), name);
    let part = cx.create_module_from_ir(buffer).map_err(|e| format!("the {name} IR is invalid: {e}"))?;
    part.set_triple(&module.get_triple());
    part.set_data_layout(&module.get_data_layout());
    module.link_in_module(part).map_err(|e| format!("linking {name} failed: {e}"))
}

/// The initial frame of a new x86-64 context: the six callee-saved registers `ctx_asm_switch` pops, then the return
/// address. `r13` holds the entry function and `r12` its argument for `ctx_boot`.
const INIT_FRAME_X86_64: &str = "  %sp = getelementptr i8, ptr %top, i64 -56
  %f_entry = getelementptr i8, ptr %sp, i64 16
  store ptr @burdin_rt_ctx_entry, ptr %f_entry
  %f_arg = getelementptr i8, ptr %sp, i64 24
  store ptr %e, ptr %f_arg
  %f_ret = getelementptr i8, ptr %sp, i64 48
  store ptr @burdin_rt_ctx_boot, ptr %f_ret";

/// The initial frame of a new arm64 context: `x19` holds the argument, `x20` the entry function, and `x30` the
/// address `ret` continues at.
const INIT_FRAME_AARCH64: &str = "  %sp = getelementptr i8, ptr %top, i64 -176
  store ptr %e, ptr %sp
  %f_entry = getelementptr i8, ptr %sp, i64 8
  store ptr @burdin_rt_ctx_entry, ptr %f_entry
  %f_ret = getelementptr i8, ptr %sp, i64 88
  store ptr @burdin_rt_ctx_boot, ptr %f_ret";

/// The module's inline assembly, which is the context switch and the first-run stub: `ctx_asm_switch(from, to)` saves
/// the callee-saved registers on the running stack, stores the stack pointer in `from`, and continues `to`;
/// `ctx_boot` is where a new context first runs, and it tells the unwinder the stack ends there.
fn switch_asm(darwin: bool, aarch64: bool) -> String {
    let underscore = if darwin { "_" } else { "" };
    let mut asm = String::new();
    let function = |name: &str| {
        if darwin {
            format!(".globl {underscore}{name}\n.p2align 4\n{underscore}{name}:\n")
        } else {
            format!(".globl {name}\n.type {name},@function\n.p2align 4\n{name}:\n")
        }
    };
    asm.push_str(".text\n");
    if aarch64 {
        asm.push_str(&function("burdin_rt_ctx_asm_switch"));
        asm.push_str(
            "    sub sp, sp, #176
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    stp d8, d9, [sp, #96]
    stp d10, d11, [sp, #112]
    stp d12, d13, [sp, #128]
    stp d14, d15, [sp, #144]
    mov x9, sp
    str x9, [x0]
    ldr x9, [x1]
    mov sp, x9
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    ldp d8, d9, [sp, #96]
    ldp d10, d11, [sp, #112]
    ldp d12, d13, [sp, #128]
    ldp d14, d15, [sp, #144]
    add sp, sp, #176
    ret
",
        );
        asm.push_str(&function("burdin_rt_ctx_boot"));
        asm.push_str(
            "    .cfi_startproc
    .cfi_undefined x30
    mov x0, x19
    blr x20
    brk #0
    .cfi_endproc
",
        );
    } else {
        asm.push_str(&function("burdin_rt_ctx_asm_switch"));
        asm.push_str(
            "    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, (%rdi)
    movq (%rsi), %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    ret
",
        );
        asm.push_str(&function("burdin_rt_ctx_boot"));
        asm.push_str(
            "    .cfi_startproc
    .cfi_undefined rip
    movq %r12, %rdi
    callq *%r13
    ud2
    .cfi_endproc
",
        );
    }
    asm
}
