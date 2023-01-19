pub use self::implementation::*;

pub mod prelude {
    pub use super::{distributions::Distribution, rngs::StdRng, CryptoRng, Rng, SeedableRng};
}

#[cfg(not(feature = "simulation"))]
mod implementation {
    pub use rand::{distributions, random, seq, thread_rng, CryptoRng, Rng, SeedableRng};
    pub use std::collections::hash_map::{DefaultHasher, RandomState};

    pub mod rngs {
        pub use rand::rngs::{OsRng, StdRng};
    }
}

#[cfg(feature = "simulation")]
mod implementation {
    pub use self::rngs::thread::thread_rng;
    pub use rand::{distributions, seq, CryptoRng, Rng, SeedableRng};

    use self::distributions::{Distribution, Standard};
    use siphasher::sip::SipHasher13;
    use std::hash::{BuildHasher, Hasher};

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
                pub(super) static THREAD_RNG: Rc<RefCell<StdRng>> =
                    Rc::new(RefCell::new(StdRng::seed_from_u64(get_seed())));
            }

            fn get_seed() -> u64 {
                // Generate/parse the seed only once per process, but log it for every thread. This
                // is so that every test logs the seed separately so that when the test fails we
                // knows what seed it was using.
                static SEED: Lazy<u64> = Lazy::new(|| {
                    const PARSE_ERROR: &str = "failed to parse OUISYNC_TEST_SEED";

                    match env::var("OUISYNC_TEST_SEED") {
                        Ok(seed) => seed.parse().expect(PARSE_ERROR),
                        Err(VarError::NotPresent) => rand::rngs::OsRng.gen(),
                        Err(VarError::NotUnicode(_)) => panic!("{}", PARSE_ERROR),
                    }
                });

                let seed = *SEED;

                tracing::debug!(target: "ouisync::rand", seed);

                seed
            }

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

    /// Drop-in replacement for `std::collections::hash_map::RandomState` which uses the seedable
    /// rng from this crate (mostly copied from std
    /// (https://doc.rust-lang.org/src/std/collections/hash/map.rs.html#3092), just modified to use
    /// the rng from this module).
    pub struct RandomState {
        k0: u64,
        k1: u64,
    }

    impl RandomState {
        pub fn new() -> Self {
            let (k0, k1): (u64, u64) = random();

            Self {
                k0: k0.wrapping_add(1),
                k1,
            }
        }
    }

    impl Default for RandomState {
        fn default() -> Self {
            Self::new()
        }
    }

    impl BuildHasher for RandomState {
        type Hasher = DefaultHasher;

        #[inline]
        fn build_hasher(&self) -> DefaultHasher {
            DefaultHasher(SipHasher13::new_with_keys(self.k0, self.k1))
        }
    }

    #[derive(Clone, Debug)]
    pub struct DefaultHasher(SipHasher13);

    impl Hasher for DefaultHasher {
        fn write(&mut self, msg: &[u8]) {
            self.0.write(msg)
        }

        fn finish(&self) -> u64 {
            self.0.finish()
        }
    }
}
