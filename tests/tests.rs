mod utils;

use self::utils::{eventually, Bin};
use std::{fs, str, thread, time::Duration};

#[test]
fn transfer_single_file() {
    let a = Bin::start(0);
    let b = Bin::start(1);

    let file_name = "foo.txt";
    let orig_content = "hello";

    fs::write(a.root().join(file_name), orig_content).unwrap();

    eventually(|| {
        let content = fs::read(b.root().join(file_name)).unwrap();
        let content = str::from_utf8(&content).unwrap();
        assert_eq!(content, orig_content);
    })
}
