use crate::sender::Sender;
use bytes::Bytes;

pub(crate) type Callback = unsafe extern "C" fn(context: *mut (), msg_ptr: *const u8, msg_len: u64);

pub(crate) struct CallbackSender {
    context: *mut (),
    callback: Callback,
}

impl CallbackSender {
    /// # Safety
    ///
    /// - `context` must be either `null` or a valid pointer to a value that is safe to send to
    ///    other threads.
    /// - `callback` must be a valid function pointer which does not leak the passed `msg_ptr`.
    pub unsafe fn new(context: *mut (), callback: Callback) -> Self {
        Self { context, callback }
    }
}

impl Sender for CallbackSender {
    fn send(&self, msg: Bytes) {
        // Safety: `self` must be created via `Self::new` and its safety contract must be uphelp
        // and it can't be modified afterwards.
        unsafe { (self.callback)(self.context, msg.as_ptr(), msg.len() as u64) }
    }
}

// Safety: The safety contract of `Self::new` must be upheld.
unsafe impl Send for CallbackSender {}
