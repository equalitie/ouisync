#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod default;

#[cfg(target_os = "android")]
pub(crate) use self::android::Logger;

#[cfg(not(target_os = "android"))]
pub(crate) use self::default::Logger;
