//! The clock behind a build's stage timings (`finish timings: …`). wasm has none
//! (`std::time::Instant::now` panics on `wasm32-unknown-unknown`), so there every instant is the
//! same and each stage reads 0 ms.

#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;

#[cfg(target_family = "wasm")]
pub use self::wasm::Instant;

#[cfg(target_family = "wasm")]
mod wasm {
    use std::time::Duration;

    /// A moment that is always now: durations between two are zero.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant;

    impl Instant {
        pub fn now() -> Self {
            Self
        }

        pub fn elapsed(&self) -> Duration {
            Duration::ZERO
        }
    }

    impl std::ops::Sub for Instant {
        type Output = Duration;

        fn sub(self, _: Self) -> Duration {
            Duration::ZERO
        }
    }
}
