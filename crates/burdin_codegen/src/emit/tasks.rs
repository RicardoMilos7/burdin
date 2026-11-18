//! The task runtime's primitives as built-in calls (`spec/tasks.md`, TK2; `spec/io.md`, IO2). Each one calls the
//! function of the same name in the runtime IR (`taskrt.rs`), which is linked in on first use.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_tir::{self as tir, IntTy, RtOp, Ty};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The runtime function behind a primitive, linking the task runtime in the first time.
    fn rt_function(&mut self, name: &str) -> GenResult<FunctionValue<'ctx>> {
        if !self.tasks_linked {
            crate::taskrt::link(self.cx, self.module, &self.triple)?;
            self.tasks_linked = true;
        }
        self.module.get_function(name).ok_or_else(|| format!("the task runtime has no `{name}`"))
    }

    /// Calls the primitive `op`. A closure argument is passed as its code and record pointers: the runtime runs it
    /// once and the closure body frees the record, so the drop glue (the third word) is not needed.
    pub(super) fn rt_primitive(&mut self, op: RtOp, args: &'a [tir::Expr]) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let name = match op {
            RtOp::ThreadStart => "burdin_rt_thread_start",
            RtOp::ThreadYield => "burdin_rt_thread_yield",
            RtOp::MutexNew => "burdin_rt_mutex_new",
            RtOp::MutexLock => "burdin_rt_mutex_lock",
            RtOp::MutexUnlock => "burdin_rt_mutex_unlock",
            RtOp::MutexFree => "burdin_rt_mutex_free",
            RtOp::CondNew => "burdin_rt_cond_new",
            RtOp::CondWait => "burdin_rt_cond_wait",
            RtOp::CondSignal => "burdin_rt_cond_signal",
            RtOp::CondBroadcast => "burdin_rt_cond_broadcast",
            RtOp::CondFree => "burdin_rt_cond_free",
            RtOp::CtxMain => "burdin_rt_ctx_main",
            RtOp::CtxNew => "burdin_rt_ctx_new",
            RtOp::CtxSwitch => "burdin_rt_ctx_switch",
            RtOp::CtxFree => "burdin_rt_ctx_free",
            RtOp::TlsGet => "burdin_rt_tls_get",
            RtOp::TlsSet => "burdin_rt_tls_set",
            RtOp::GlobalGet => "burdin_rt_global_get",
            RtOp::GlobalSet => "burdin_rt_global_set",
            RtOp::SpinLock => "burdin_rt_spin_lock",
            RtOp::SpinUnlock => "burdin_rt_spin_unlock",
            RtOp::ConfigGet => "burdin_rt_config_get",
            RtOp::ConfigSet => "burdin_rt_config_set",
            RtOp::CpuCount => "burdin_rt_cpu_count",
            RtOp::IoNew => "burdin_rt_io_new",
            RtOp::IoWatch => "burdin_rt_io_watch",
            RtOp::IoUnwatch => "burdin_rt_io_unwatch",
            RtOp::IoPoll => "burdin_rt_io_poll",
            RtOp::IoWake => "burdin_rt_io_wake",
            RtOp::IoGlobalGet => "burdin_rt_io_global_get",
            RtOp::IoGlobalSet => "burdin_rt_io_global_set",
            RtOp::PoolSubmit | RtOp::PoolSubmitRaw => "burdin_rt_pool_submit",
            RtOp::NetSocket => "burdin_rt_net_socket",
            RtOp::NetBind => "burdin_rt_net_bind",
            RtOp::NetListen => "burdin_rt_net_listen",
            RtOp::NetAccept => "burdin_rt_net_accept",
            RtOp::NetConnect => "burdin_rt_net_connect",
            RtOp::NetSend => "burdin_rt_net_send",
            RtOp::NetRecv => "burdin_rt_net_recv",
            RtOp::NetSendTo => "burdin_rt_net_send_to",
            RtOp::NetRecvFrom => "burdin_rt_net_recv_from",
            RtOp::NetClose => "burdin_rt_net_close",
            RtOp::NetShutdown => "burdin_rt_net_shutdown",
            RtOp::NetError => "burdin_rt_net_error",
            RtOp::NetSetNoDelay => "burdin_rt_net_set_nodelay",
            RtOp::NetLocalAddr => "burdin_rt_net_local_addr",
            RtOp::NetPeerAddr => "burdin_rt_net_peer_addr",
            RtOp::NetLastPeer => "burdin_rt_net_last_peer",
            RtOp::NetLastError => "burdin_rt_net_last_error",
        };
        let function = self.rt_function(name)?;
        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            if op.takes_closure() && i + 1 == args.len() {
                let value = self.expr_mode(arg, Mode::Move)?;
                let value = self.need(value, &arg.ty);
                let closure = value.into_struct_value();
                let code = self.builder.build_extract_value(closure, 0, "code").map_err(llvm_err)?;
                let env = self.builder.build_extract_value(closure, 1, "env").map_err(llvm_err)?;
                if op == RtOp::ThreadStart
                    && let Some(untrack) = self.rt.untrack
                {
                    // A thread may run until the process ends, so the leak check of a test must not count the record
                    // its closure lives in.
                    self.builder.build_call(untrack, &[env.into()], "").map_err(llvm_err)?;
                }
                values.push(code.into());
                values.push(env.into());
                continue;
            }
            match op {
                // The socket buffer is the caller's list: the runtime appends to it after growing it
                // (`spec/io.md`, IO5), exactly like the file primitives.
                RtOp::NetRecv | RtOp::NetRecvFrom if i == 1 => {
                    let Some(ptr) = self.place_ptr(arg)? else {
                        return Err("the socket read buffer is not a place".into());
                    };
                    values.push(ptr.into());
                }
                // A `str` host is a pair of a pointer and a length, and a byte slice the same.
                RtOp::NetBind | RtOp::NetConnect if i == 1 => {
                    let (ptr, len) = self.str_parts(arg)?;
                    values.push(ptr.into());
                    values.push(len.into());
                }
                RtOp::NetSend if i == 1 => {
                    let slice = self.expr_mode(arg, Mode::Use)?;
                    let slice = self.need(slice, &Ty::Slice(Box::new(Ty::Int(IntTy::U8)))).into_struct_value();
                    let ptr = self.builder.build_extract_value(slice, 0, "").map_err(llvm_err)?;
                    let len = self.builder.build_extract_value(slice, 1, "").map_err(llvm_err)?;
                    values.push(ptr.into());
                    values.push(len.into());
                }
                RtOp::NetSendTo if i == 1 => {
                    let slice = self.expr_mode(arg, Mode::Use)?;
                    let slice = self.need(slice, &Ty::Slice(Box::new(Ty::Int(IntTy::U8)))).into_struct_value();
                    let ptr = self.builder.build_extract_value(slice, 0, "").map_err(llvm_err)?;
                    let len = self.builder.build_extract_value(slice, 1, "").map_err(llvm_err)?;
                    values.push(ptr.into());
                    values.push(len.into());
                }
                RtOp::NetSendTo if i == 2 => {
                    let (ptr, len) = self.str_parts(arg)?;
                    values.push(ptr.into());
                    values.push(len.into());
                }
                _ => {
                    let value = self.expr_mode(arg, Mode::Use)?;
                    let value = self.need(value, &arg.ty);
                    values.push(value.into());
                }
            }
        }
        let call = self.builder.build_call(function, &values, "").map_err(llvm_err)?;
        Ok(call.try_as_basic_value().basic())
    }
}
