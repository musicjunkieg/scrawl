//! Work spread over threads natively and run in turn on wasm, which has no threads
//! (`std::thread::spawn` fails there). Results come back in order either way, so a build gives
//! the same bundle on both.

use std::thread::Result;

/// Runs `f` on every item, each on its own thread, and returns the results in the items' order.
/// A panic in `f` is that item's `Err`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn map<T: Send, R: Send>(
    items: impl IntoIterator<Item = T>,
    f: impl Fn(T) -> R + Sync,
) -> Vec<Result<R>> {
    let f = &f;
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .into_iter()
            .map(|item| scope.spawn(move || f(item)))
            .collect();
        workers.into_iter().map(|w| w.join()).collect()
    })
}

#[cfg(target_family = "wasm")]
pub(crate) fn map<T: Send, R: Send>(
    items: impl IntoIterator<Item = T>,
    f: impl Fn(T) -> R + Sync,
) -> Vec<Result<R>> {
    items
        .into_iter()
        .map(|item| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(item))))
        .collect()
}

/// Runs `a` on another thread while `b` runs on this one. A panic in `a` is its `Err`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn join<A: Send, B>(
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B,
) -> (Result<A>, B) {
    std::thread::scope(|scope| {
        let a = scope.spawn(a);
        let b = b();
        (a.join(), b)
    })
}

#[cfg(target_family = "wasm")]
pub(crate) fn join<A: Send, B>(
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B,
) -> (Result<A>, B) {
    let a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(a));
    (a, b())
}

#[cfg(test)]
mod tests {
    #[test]
    fn results_keep_the_items_order_and_panics_stay_with_their_item() {
        let out = super::map(0..8_u32, |i| {
            assert!(i != 5, "five");
            i * 10
        });
        assert_eq!(out.len(), 8);
        for (i, r) in out.into_iter().enumerate() {
            match i {
                5 => assert!(r.is_err()),
                _ => assert_eq!(r.unwrap(), i as u32 * 10),
            }
        }
        let (a, b) = super::join(|| 1, || 2);
        assert_eq!((a.unwrap(), b), (1, 2));
    }
}
