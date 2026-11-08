//! A stack as big as Linux's and macOS's main thread for the UI, on the thread it is called on (see
//! "スタック" in `docs/design/39-platform-windows.md`).

use std::any::Any;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use windows::Win32::System::Threading::{
    ConvertFiberToThread, ConvertThreadToFiber, CreateFiberEx, DeleteFiber, IsThreadAFiber,
    SwitchToFiber,
};

/// What the main threads of Linux and macOS have.
pub const UI_STACK: usize = 8 << 20;

struct Call<'a, R> {
    f: Option<Box<dyn FnOnce() -> R + 'a>>,
    result: Option<Result<R, Box<dyn Any + Send>>>,
    back: *mut c_void,
}

/// Runs `f` on a stack of `size` bytes, on this thread: its windows, messages and thread-locals
/// stay what they are. Windows gives the main thread 1 MiB, which deep views in debug builds go
/// beyond.
pub fn with_stack<R>(size: usize, f: impl FnOnce() -> R) -> R {
    unsafe extern "system" fn start<R>(param: *mut c_void) {
        // SAFETY: what `with_stack` passed, which outlives the fiber.
        let call = unsafe { &mut *(param as *mut Call<'_, R>) };
        let f = call.f.take().expect("run once");
        call.result = Some(catch_unwind(AssertUnwindSafe(f)));
        // SAFETY: the fiber that switched here, which waits for this.
        unsafe { SwitchToFiber(call.back) };
    }

    // SAFETY: fibers of this thread only; this one is switched back to before anything else.
    unsafe {
        // A thread that is a fiber already is someone else's: it stays as it is.
        if IsThreadAFiber().as_bool() {
            return f();
        }
        let back = ConvertThreadToFiber(None);
        if back.is_null() {
            return f();
        }
        let mut call = Call {
            f: Some(Box::new(f)),
            result: None,
            back,
        };
        let fiber = CreateFiberEx(
            0,
            size,
            0,
            Some(start::<R>),
            Some(&mut call as *mut Call<'_, R> as *const c_void),
        );
        if fiber.is_null() {
            let _ = ConvertFiberToThread();
            let f = call.f.take().expect("not run");
            return f();
        }
        SwitchToFiber(fiber);
        DeleteFiber(fiber);
        let _ = ConvertFiberToThread();
        match call.result.take().expect("the fiber ran") {
            Ok(r) => r,
            Err(panic) => resume_unwind(panic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_on_the_same_thread_with_a_big_stack() {
        let here = std::thread::current().id();
        let n = with_stack(UI_STACK, || {
            // More than 1 MiB on the stack.
            let big = [1u8; 4 << 20];
            assert_eq!(std::thread::current().id(), here);
            std::hint::black_box(&big)
                .iter()
                .map(|&b| b as usize)
                .sum::<usize>()
        });
        assert_eq!(n, 4 << 20);
    }

    #[test]
    fn panics_come_through() {
        let r = std::panic::catch_unwind(|| with_stack(1 << 20, || panic!("inside")));
        assert!(r.is_err());
        // The thread is itself again.
        assert_eq!(with_stack(1 << 20, || 7), 7);
    }
}
