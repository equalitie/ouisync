use ouisync_lib::EntryType;

pub const ENTRY_TYPE_FILE: u8 = 1;
pub const ENTRY_TYPE_DIRECTORY: u8 = 2;

pub const ACCESS_MODE_BLIND: u8 = 0;
pub const ACCESS_MODE_READ: u8 = 1;
pub const ACCESS_MODE_WRITE: u8 = 2;

pub(crate) fn entry_type_to_num(entry_type: EntryType) -> u8 {
    match entry_type {
        EntryType::File => ENTRY_TYPE_FILE,
        EntryType::Directory => ENTRY_TYPE_DIRECTORY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync_lib::AccessMode;

    #[test]
    fn access_mode_constants() {
        for (mode, num) in [
            (AccessMode::Blind, ACCESS_MODE_BLIND),
            (AccessMode::Read, ACCESS_MODE_READ),
            (AccessMode::Write, ACCESS_MODE_WRITE),
        ] {
            assert_eq!(u8::from(mode), num);
            assert_eq!(AccessMode::try_from(num).unwrap(), mode);
        }
    }
}
