//! The JavaScript engine, behind as little as can be: running a script, calling a function with
//! text and getting text back, and one function the scripts call to ask the viewer something.
//! Another engine only has to do the same. See "JavaScript" in `docs/design/26-pdf.md`.

use rquickjs::{Context, Ctx, Error, Function, Runtime};

/// How deep scripts can call, in bytes of the stack of the thread they run on.
const STACK: usize = 1 << 20;

pub(crate) struct Js {
    // The context goes before the runtime it is in.
    context: Context,
    _runtime: Runtime,
}

impl Js {
    /// `stop` is asked now and then while a script runs, and stops it if it says so. `ask` is
    /// `__ask(text)` for the scripts, which waits for what it gives back.
    pub fn new(
        memory: usize,
        stop: impl FnMut() -> bool + 'static,
        ask: impl Fn(String) -> String + 'static,
    ) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(memory);
        runtime.set_max_stack_size(STACK);
        runtime.set_interrupt_handler(Some(Box::new(stop)));
        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        context.with(|ctx| {
            let f = Function::new(ctx.clone(), move |text: String| ask(text))
                .and_then(|f| ctx.globals().set("__ask", f));
            f.map_err(|e| describe(&ctx, e))
        })?;
        Ok(Self {
            context,
            _runtime: runtime,
        })
    }

    /// Runs a script in the global scope.
    pub fn eval(&self, source: &str) -> Result<(), String> {
        self.context
            .with(|ctx| ctx.eval::<(), _>(source).map_err(|e| describe(&ctx, e)))
    }

    /// How many bytes the scripts take.
    pub fn memory(&self) -> usize {
        self._runtime.memory_usage().memory_used_size.max(0) as usize
    }

    /// Calls a global function with text, which gives text back.
    pub fn call(&self, function: &str, argument: &str) -> Result<String, String> {
        self.context.with(|ctx| {
            let f: Function = ctx.globals().get(function).map_err(|e| describe(&ctx, e))?;
            f.call::<_, String>((argument,))
                .map_err(|e| describe(&ctx, e))
        })
    }
}

/// What went wrong, with what the script threw.
fn describe(ctx: &Ctx<'_>, error: Error) -> String {
    if !matches!(error, Error::Exception) {
        return error.to_string();
    }
    let thrown = ctx.catch();
    if let Some(exception) = thrown.as_exception() {
        let message = exception.message().unwrap_or_default();
        return match exception.stack() {
            Some(stack) if !stack.is_empty() => format!("{message}\n{stack}"),
            _ => message,
        };
    }
    thrown
        .as_string()
        .and_then(|s| s.to_string().ok())
        .unwrap_or_else(|| format!("{thrown:?}"))
}
