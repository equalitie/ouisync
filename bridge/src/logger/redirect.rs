//! stdout and stderr redirection

use std::io;
use sys::OwnedDescriptor;
pub(crate) use sys::{AsDescriptor, SetDescriptor};

pub(crate) struct Redirect<S, D>
where
    S: AsDescriptor + SetDescriptor,
    D: AsDescriptor,
{
    src: S,
    src_orig: OwnedDescriptor,
    _dst: D,
}

impl<S, D> Redirect<S, D>
where
    S: AsDescriptor + SetDescriptor,
    D: AsDescriptor,
{
    pub fn new(src: S, dst: D) -> io::Result<Self> {
        // Remember the old fd so we can point it to where it pointed before when we are done.
        let src_orig = src.as_descriptor().try_clone_to_owned()?;

        src.set_descriptor(dst.as_descriptor())?;

        Ok(Self {
            src,
            src_orig,
            _dst: dst,
        })
    }

    // Returns a file representing the original, unredirected stream.
    #[cfg(not(target_os = "android"))] // not used on android
    pub fn orig(&self) -> io::Result<std::fs::File> {
        Ok(self.src_orig.try_clone()?.into())
    }
}

impl<S, D> Drop for Redirect<S, D>
where
    S: AsDescriptor + SetDescriptor,
    D: AsDescriptor,
{
    fn drop(&mut self) {
        if let Err(error) = self.src.set_descriptor(self.src_orig.as_descriptor()) {
            tracing::error!(
                ?error,
                "Failed to point the redirected descriptor to its original target"
            );
        }
    }
}

#[cfg(unix)]
mod sys {
    use std::{
        io,
        os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    };

    pub(crate) type OwnedDescriptor = OwnedFd;
    pub(crate) type BorrowedDescriptor<'a> = BorrowedFd<'a>;

    pub(crate) trait AsDescriptor: AsFd {
        fn as_descriptor(&self) -> BorrowedDescriptor<'_>;
    }

    impl<T> AsDescriptor for T
    where
        T: AsFd,
    {
        fn as_descriptor(&self) -> BorrowedDescriptor<'_> {
            AsFd::as_fd(self)
        }
    }

    pub(crate) trait SetDescriptor {
        fn set_descriptor(&self, d: BorrowedDescriptor<'_>) -> io::Result<()>;
    }

    impl<T> SetDescriptor for T
    where
        T: AsFd,
    {
        fn set_descriptor(&self, d: BorrowedDescriptor<'_>) -> io::Result<()> {
            // SAFETY: Both file descriptors are valid because they are obtained using `as_raw_fd`
            // from valid io objects.
            unsafe {
                if libc::dup2(d.as_raw_fd(), self.as_fd().as_raw_fd()) >= 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::{
        io::{Stderr, Stdout},
        os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle},
    };
    use winapi::shared::minwindef::DWORD;
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    pub(crate) type OwnedDescriptor = OwnedHandle;
    pub(crate) type BorrowedDescriptor<'a> = BorrowedHandle<'a>;

    pub(crate) trait AsDescriptor: AsHandle {
        fn as_descriptor(&self) -> BorrowedDescriptor<'_>;
    }

    impl<T> AsDescriptor for T
    where
        T: AsHandle,
    {
        fn as_descriptor(&self) -> BorrowedDescriptor<'_> {
            AsHandle::as_handle(self)
        }
    }

    pub(crate) trait SetDescriptor {
        fn set_descriptor(&self, d: BorrowedDescriptor<'_>) -> io::Result<()>;
    }

    impl SetDescriptor for Stdout {
        fn set_descriptor(&self, d: BorrowedDescriptor<'_>) -> io::Result<()> {
            unsafe { set_std_handle(STD_OUTPUT_HANDLE, d) }
        }
    }

    impl SetDescriptor for Stderr {
        fn set_descriptor(&self, d: BorrowedDescriptor<'_>) -> io::Result<()> {
            unsafe { set_std_handle(STD_ERROR_HANDLE, d) }
        }
    }

    unsafe fn set_std_handle(device: DWORD, d: BorrowedDescriptor<'_>) -> io::Result<()> {
        if winapi::um::processenv::SetStdHandle(device, d.as_raw_handle()) != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
