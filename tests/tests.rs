mod utils;

use self::utils::{eventually, Bin};
use std::fs;

#[test]
fn transfer_single_file() {
    let a = Bin::start(0);
    let b = Bin::start(1);

    let file_name = "foo.txt";
    let orig_content = "hello";

    fs::write(a.root().join(file_name), orig_content).unwrap();

    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name)).unwrap();
        assert_eq!(content, orig_content);
    })
}

// FIXME: this test currently fails
#[ignore]
#[test]
fn sequential_write_to_the_same_file() {
    let a = Bin::start(0);
    let b = Bin::start(1);

    let file_name = "bar.txt";
    let content_a = "hello from A";
    let content_b = "hello from B";

    // A writes first
    fs::write(a.root().join(file_name), &content_a).unwrap();

    // B reads what A wrote
    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name)).unwrap();
        assert_eq!(content, content_a);
    });

    // B writes
    fs::write(b.root().join(file_name), &content_b).unwrap();

    // A reads what B wrote
    eventually(|| {
        let content = fs::read_to_string(a.root().join(file_name)).unwrap();
        assert_eq!(content, content_b);
    });
}
