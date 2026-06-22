use futures::FutureExt;
use tower_lsp::jsonrpc::Result;

/// Wraps an async handler in `catch_unwind` so a panic in one request doesn't
/// kill the server process. Returns an internal error to the client on panic.
pub(crate) async fn panic_safe<F, T>(method: &str, future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>> + Send,
    T: Send + 'static,
{
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    // Wrapper that sets PANIC_CAUGHT on each poll so the panic hook
    // always sees the flag regardless of which thread resumes the future.
    struct PanicGuarded<Fut>(Pin<Box<Fut>>);

    impl<Fut: Future> Future for PanicGuarded<Fut> {
        type Output = Fut::Output;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            crate::PANIC_CAUGHT.with(|c| c.set(true));
            let result = self.0.as_mut().poll(cx);
            if result.is_ready() {
                crate::PANIC_CAUGHT.with(|c| c.set(false));
            }
            result
        }
    }

    impl<Fut> Drop for PanicGuarded<Fut> {
        fn drop(&mut self) {
            crate::PANIC_CAUGHT.with(|c| c.set(false));
        }
    }

    let started = std::time::Instant::now();
    let guarded = PanicGuarded(Box::pin(future));
    let result = std::panic::AssertUnwindSafe(guarded).catch_unwind().await;
    let elapsed_ms = started.elapsed().as_millis();
    if elapsed_ms > 400 {
        log::warn!("SLOW request {method}: {elapsed_ms}ms");
    }

    match result {
        Ok(result) => result,
        Err(payload) => {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_owned()
            };
            log::error!("PANIC in {method}: {message}");
            Err(tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: format!("internal error in {method}").into(),
                data: None,
            })
        }
    }
}
