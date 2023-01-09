pub use self::implementation::*;

pub mod prelude {
    pub use super::{distributions::Distribution, rngs::StdRng, CryptoRng, Rng, SeedableRng};
}

#[cfg(not(feature = "simulation"))]
mod implementation {
    pub use rand::{distributions, random, seq, thread_rng, CryptoRng, Rng, SeedableRng};

    pub mod rngs {
        pub use rand::rngs::{OsRng, StdRng};
    }
}

#[cfg(feature = "simulation")]
mod implementation {
    pub use self::rngs::thread::thread_rng;
    pub use rand::{distributions, seq, CryptoRng, Rng, SeedableRng};

    use self::distributions::{Distribution, Standard};

    pub mod rngs {
        pub use self::{os::OsRng, thread::ThreadRng};
        pub use rand::rngs::StdRng;

        /// ThreadRng
        pub(super) mod thread {
            use once_cell::sync::Lazy;
            use rand::{rngs::StdRng, CryptoRng, Error, Rng, RngCore, SeedableRng};
            use std::{
                cell::RefCell,
                env::{self, VarError},
                rc::Rc,
            };

            thread_local! {
                pub(super) static THREAD_RNG: Rc<RefCell<StdRng>> = Rc::new(RefCell::new(StdRng::seed_from_u64(*SEED)));
            }

            static SEED: Lazy<u64> = Lazy::new(|| {
                const PARSE_ERROR: &str = "failed to parse OUISYNC_TEST_SEED";

                let seed = match env::var("OUISYNC_TEST_SEED") {
                    Ok(seed) => seed.parse().expect(PARSE_ERROR),
                    Err(VarError::NotPresent) => rand::rngs::OsRng.gen(),
                    Err(VarError::NotUnicode(_)) => panic!("{}", PARSE_ERROR),
                };

                tracing::debug!(target: "ouisync::rand", seed);

                seed
            });

            pub struct ThreadRng(Rc<RefCell<StdRng>>);

            impl RngCore for ThreadRng {
                fn next_u32(&mut self) -> u32 {
                    self.0.borrow_mut().next_u32()
                }

                fn next_u64(&mut self) -> u64 {
                    self.0.borrow_mut().next_u64()
                }

                fn fill_bytes(&mut self, dest: &mut [u8]) {
                    self.0.borrow_mut().fill_bytes(dest)
                }

                fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
                    self.0.borrow_mut().try_fill_bytes(dest)
                }
            }

            impl CryptoRng for ThreadRng {}

            pub fn thread_rng() -> ThreadRng {
                THREAD_RNG.with(|inner| ThreadRng(inner.clone()))
            }
        }

        /// OsRng
        pub(super) mod os {
            use super::thread::thread_rng;
            use rand::{CryptoRng, Error, RngCore};

            pub struct OsRng;

            // The simulated OsRng is just ThreadRng under the hood
            impl RngCore for OsRng {
                fn next_u32(&mut self) -> u32 {
                    thread_rng().next_u32()
                }

                fn next_u64(&mut self) -> u64 {
                    thread_rng().next_u64()
                }

                fn fill_bytes(&mut self, dest: &mut [u8]) {
                    thread_rng().fill_bytes(dest)
                }

                fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
                    thread_rng().try_fill_bytes(dest)
                }
            }

            impl CryptoRng for OsRng {}
        }
    }

    pub fn random<T>() -> T
    where
        Standard: Distribution<T>,
    {
        thread_rng().gen()
    }
}
