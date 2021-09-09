mod utils;

use self::utils::{eventually, Bin};
use parking_lot::Mutex;
use std::fs;

// HACK: mutex to make sure the integration tests run sequentially, so they don't interfere with
//       each other.
// TODO: disable local discovery in these tests and establish explicit connections instead to avoid
//       this interference. Then remove this wrapper.
// NOTE: using `parking_lot::Mutex` instead of `std::Mutex` because it can be constructed with a
//       `const` function which makes it easier to use in a `static` (no need for `unsafe`).
static MUTEX: Mutex<()> = parking_lot::const_mutex(());

#[test]
fn transfer_single_file() {
    let _guard = MUTEX.lock();

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

#[test]
fn sequential_write_to_the_same_file() {
    let _guard = MUTEX.lock();

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
